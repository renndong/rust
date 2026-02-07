// #![allow(dead_code)]
// #![allow(warnings)]

use rustc_abi::ExternAbi;
use rustc_hir::Attribute;
use rustc_hir::attrs::AttributeKind;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{self, *};
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::DUMMY_SP;
use rustc_span::source_map::{Spanned, dummy_spanned};
use rustc_span::symbol::{Symbol, sym};

pub(super) struct FFISanitizer;

impl<'tcx> crate::MirPass<'tcx> for FFISanitizer {
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        if !tcx.features().ffi_sanitizer() {
            return;
        }
        if has_ffi_call(tcx, body) {
            println!("FFISanitizer");
            instrument_func_with_ffi_call(tcx, body);
        } else if can_be_ffi_call(tcx, body) {
            println!("FFISanitizer");
            instrument_func_can_be_ffi_call(tcx, body);
        }
        if is_main_function(tcx, body) {
            instrument_main_func(tcx, body);
        }
    }

    fn is_required(&self) -> bool {
        true
    }
}

fn is_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>, func: &Operand<'tcx>) -> bool {
    if let ty::FnDef(def_id, _) = func.ty(body, tcx).kind() {
        return tcx.is_foreign_item(*def_id);
    }
    return false;
}

fn has_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>) -> bool {
    for (_, block) in body.basic_blocks.iter_enumerated() {
        let Some(terminator) = &block.terminator else { continue };
        if let TerminatorKind::Call { func, .. } = &terminator.kind {
            if is_ffi_call(tcx, body, func) {
                return true;
            }
        }
    }
    return false;
}

fn can_be_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>) -> bool {
    let def_id = body.source.def_id().expect_local();

    let mut no_mangle = tcx.has_attr(def_id, sym::no_mangle);
    for attr in tcx.get_all_attrs(def_id).iter() {
        if let Attribute::Parsed(parsed) = attr {
            no_mangle |= match parsed {
                AttributeKind::NoMangle(_) => true,
                AttributeKind::ExportName { .. } => true,
                _ => false,
            };
        }
    }

    if no_mangle
        && tcx.visibility(def_id).is_public()
        && let ExternAbi::C { .. } = tcx.fn_sig(def_id).skip_binder().abi()
    {
        return true;
    }
    return false;
}

fn is_main_function<'tcx>(tcx: TyCtxt<'_>, body: &Body<'tcx>) -> bool {
    let def_id = body.source.def_id().expect_local();

    if let Some((entry_def_id, _)) = tcx.entry_fn(()) {
        entry_def_id == def_id.to_def_id()
    } else {
        false
    }
}

fn get_temp_local<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) -> Place<'tcx> {
    let local_decl = LocalDecl::new(tcx.types.unit, DUMMY_SP);
    let local = body.local_decls.push(local_decl);
    Place::from(local)
}

fn instrument_func_with_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    for current in START_BLOCK..body.basic_blocks.next_index() {
        match &body[current].terminator().kind {
            TerminatorKind::Call { func, .. } => {
                if is_ffi_call(tcx, body, func) {
                    instrument_ffi_call(tcx, body, current)
                } else {
                    instrument_ptr_use_term(tcx, body, current);
                }
            }
            TerminatorKind::Return { .. } => instrument_return(tcx, body, current),
            TerminatorKind::TailCall { .. } => instrument_ptr_use_term(tcx, body, current),
            TerminatorKind::Drop { .. } => instrument_drop(tcx, body, current), // may never execute
            _ => {}
        }

        let len = body[current].statements.len();
        for statement_index in (0..len).rev() {
            let loc = Location { block: current, statement_index };

            let mut visitor = DerefVisitor { vars: Vec::new() };
            visitor.visit_statement(&body[current].statements[statement_index], loc);

            let mut args = Vec::new();
            while let Some(arg) = visitor.vars.pop() {
                if matches!(arg.ty(&body.local_decls, tcx).ty.kind(), ty::RawPtr(..)) {
                    println!("pointer deref {:?}", arg);
                    args.push(arg);
                }
            }

            if !args.is_empty() {
                let stmts = body[current].statements.split_off(statement_index);
                instrument_ptr_use_stmt(tcx, body, current, stmts, args);
            }
        }
    }
}

fn select_raw_ptr_args<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut Body<'tcx>,
    args: &Box<[Spanned<Operand<'tcx>>]>,
) -> Vec<(Ty<'tcx>, Spanned<Operand<'tcx>>)> {
    let mut func_args = Vec::new();
    for arg in args.iter() {
        match &arg.node {
            Operand::Copy(place) | Operand::Move(place) => {
                let base_place = mir::Place { local: place.local, projection: ty::List::empty() };
                if let ty::RawPtr(pty, _) = base_place.ty(&body.local_decls, tcx).ty.kind() {
                    func_args.push((pty.clone(), arg.clone()));
                }
            }
            _ => {}
        }
    }
    return func_args;
}

fn instrument_return<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>, current: BasicBlock) {
    let ty::RawPtr(pty, _) = body.return_ty().kind() else { return };
    let ret_place = Place::return_place();

    let pre_cond_fn_name = "ffi_sanitizer_use_pre_cond";
    let pre_cond_fn_symbol = Symbol::intern(pre_cond_fn_name);
    let Some(pre_cond_fn_def_id) = tcx.get_diagnostic_item(pre_cond_fn_symbol) else { return };

    let block = BasicBlockData::new(Some(body[current].terminator().clone()), false);
    let ret_bb = body.basic_blocks_mut().push(block);

    let source_info = body[current].terminator().source_info;
    let pre_cond_fn =
        Operand::function_handle(tcx, pre_cond_fn_def_id, [pty.clone().into()], source_info.span);

    let fn_arg = dummy_spanned(Operand::Copy(ret_place));
    body[current].terminator_mut().kind = TerminatorKind::Call {
        func: pre_cond_fn,
        args: [fn_arg].into(),
        destination: get_temp_local(tcx, body),
        target: Some(ret_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
}

fn instrument_ptr_use_term<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>, current: BasicBlock) {
    let pre_cond_args = match body[current].terminator().kind.clone() {
        TerminatorKind::Call { args, .. } | TerminatorKind::TailCall { args, .. } => {
            select_raw_ptr_args(tcx, body, &args)
        }
        _ => Vec::new(),
    };

    if pre_cond_args.is_empty() {
        return;
    }

    let source_info = body[current].terminator().source_info;
    let pre_cond_fn_name = "ffi_sanitizer_use_pre_cond";
    let pre_cond_fn_symbol = Symbol::intern(pre_cond_fn_name);
    let Some(pre_cond_fn_def_id) = tcx.get_diagnostic_item(pre_cond_fn_symbol) else { return };

    let mut pre_cond_bb = Vec::new();
    for (generic_arg, func_arg) in pre_cond_args.iter().cloned() {
        let pre_cond_fn = Operand::function_handle(
            tcx,
            pre_cond_fn_def_id,
            [generic_arg.into()],
            source_info.span,
        );

        let terminator = Terminator {
            source_info,
            kind: TerminatorKind::Call {
                func: pre_cond_fn,
                args: [func_arg.into()].into(),
                destination: get_temp_local(tcx, body),
                target: Some(BasicBlock::ZERO),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: DUMMY_SP,
            },
        };

        let block = BasicBlockData::new(Some(terminator), false);
        let bb = body.basic_blocks_mut().push(block);
        pre_cond_bb.push(bb);
    }

    let block = BasicBlockData::new(Some(body[current].terminator().clone()), false);
    let last_bb = body.basic_blocks_mut().push(block);

    if let Some(terminator) = body[current].terminator.as_mut() {
        *terminator = Terminator {
            source_info,
            kind: TerminatorKind::Goto { target: *pre_cond_bb.first().unwrap() },
        };
    }

    for (idx, &pre_bb) in pre_cond_bb.iter().enumerate() {
        let target = if let Some(val) = pre_cond_bb.get(idx + 1) { *val } else { last_bb };
        body[pre_bb].terminator_mut().successors_mut(|bb| *bb = target);
    }
}

fn instrument_ptr_use_stmt<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut Body<'tcx>,
    current: BasicBlock,
    stmts: Vec<Statement<'tcx>>,
    args: Vec<Place<'tcx>>,
) {
    let source_info = body[current].terminator().source_info;
    let pre_cond_fn_name = "ffi_sanitizer_use_pre_cond";
    let pre_cond_fn_symbol = Symbol::intern(pre_cond_fn_name);
    let Some(pre_cond_fn_def_id) = tcx.get_diagnostic_item(pre_cond_fn_symbol) else { return };

    let mut pre_cond_bb = Vec::new();
    for arg in args.iter().cloned() {
        let ty::RawPtr(pty, _) = arg.ty(&body.local_decls, tcx).ty.kind() else {
            panic!("not raw pointer");
        };

        let pre_cond_fn = Operand::function_handle(
            tcx,
            pre_cond_fn_def_id,
            [pty.clone().into()],
            source_info.span,
        );

        let terminator = Terminator {
            source_info,
            kind: TerminatorKind::Call {
                func: pre_cond_fn,
                args: [dummy_spanned(Operand::Copy(arg))].into(),
                destination: get_temp_local(tcx, body),
                target: Some(BasicBlock::ZERO),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Misc,
                fn_span: DUMMY_SP,
            },
        };

        let block = BasicBlockData::new(Some(terminator), false);
        let bb = body.basic_blocks_mut().push(block);
        pre_cond_bb.push(bb);
    }

    let block = BasicBlockData::new_stmts(stmts, Some(body[current].terminator().clone()), false);
    let last_bb = body.basic_blocks_mut().push(block);

    if let Some(terminator) = body[current].terminator.as_mut() {
        *terminator = Terminator {
            source_info,
            kind: TerminatorKind::Goto { target: *pre_cond_bb.first().unwrap() },
        };
    }

    for (idx, &pre_bb) in pre_cond_bb.iter().enumerate() {
        let target = if let Some(val) = pre_cond_bb.get(idx + 1) { *val } else { last_bb };
        body[pre_bb].terminator_mut().successors_mut(|bb| *bb = target);
    }
}

fn instrument_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>, current: BasicBlock) {
    let TerminatorKind::Call { func, args, destination, target, unwind, call_source, fn_span } =
        body[current].terminator().kind.clone()
    else {
        return;
    };

    // get pre condition
    let pre_cond_fn_name = "ffi_sanitizer_ffi_pre_cond";
    let pre_cond_fn_symbol = Symbol::intern(pre_cond_fn_name);
    let Some(pre_cond_fn_def_id) = tcx.get_diagnostic_item(pre_cond_fn_symbol) else { return };

    // get post condition
    let post_cond_fn_name = "ffi_sanitizer_ffi_post_cond";
    let post_cond_fn_symbol = Symbol::intern(post_cond_fn_name);
    let Some(post_cond_fn_def_id) = tcx.get_diagnostic_item(post_cond_fn_symbol) else { return };

    let pre_cond_args = select_raw_ptr_args(tcx, body, &args);
    let mut post_cond_args = Vec::new();

    for (generic_arg, func_arg) in pre_cond_args.iter().cloned() {
        post_cond_args.push((generic_arg, func_arg, false));
    }
    if let ty::RawPtr(pty, _) = destination.ty(&body.local_decls, tcx).ty.kind() {
        post_cond_args.push((*pty, dummy_spanned(Operand::Copy(destination)), true));
    }

    if pre_cond_args.is_empty() && post_cond_args.is_empty() {
        return;
    }

    let source_info = body[current].terminator().source_info;

    // create basic blocks for pre condition function
    let mut pre_cond_bb = Vec::new();
    for (generic_arg, func_arg) in pre_cond_args.iter().cloned() {
        let pre_cond_fn = Operand::function_handle(
            tcx,
            pre_cond_fn_def_id,
            [generic_arg.into()],
            source_info.span,
        );

        let terminator = Terminator {
            source_info,
            kind: TerminatorKind::Call {
                func: pre_cond_fn,
                args: [func_arg.into()].into(),
                destination: get_temp_local(tcx, body),
                target: Some(BasicBlock::ZERO),
                unwind,
                call_source,
                fn_span,
            },
        };

        let block = BasicBlockData::new(Some(terminator), false);
        let bb = body.basic_blocks_mut().push(block);
        pre_cond_bb.push(bb);
    }

    // create a basic block for the ffi call
    let terminator = Terminator {
        source_info,
        kind: TerminatorKind::Call {
            func: func.clone(),
            args: args.clone(),
            destination,
            target: Some(BasicBlock::ZERO),
            unwind,
            call_source,
            fn_span,
        },
    };
    let ffi_call_bb = body.basic_blocks_mut().push(BasicBlockData::new(Some(terminator), false));

    // create basic blocks for post condition function
    let mut post_cond_bb = Vec::new();
    for (generic_arg, func_arg, is_return_val) in post_cond_args.iter().cloned() {
        let post_cond_fn = Operand::function_handle(
            tcx,
            post_cond_fn_def_id,
            [generic_arg.into()],
            source_info.span,
        );

        let const_false = Operand::const_from_scalar(
            tcx,
            tcx.types.bool,
            Scalar::from_bool(is_return_val),
            source_info.span,
        );

        let terminator = Terminator {
            source_info,
            kind: TerminatorKind::Call {
                func: post_cond_fn,
                args: [func_arg.into(), dummy_spanned(const_false)].into(),
                destination: get_temp_local(tcx, body),
                target: Some(BasicBlock::ZERO),
                unwind,
                call_source,
                fn_span,
            },
        };

        let block = BasicBlockData::new(Some(terminator), false);
        let bb = body.basic_blocks_mut().push(block);
        post_cond_bb.push(bb);
    }

    /*
     * next we link the new created basic block
     */

    // set a terminator goto the first pre cond block
    if let Some(terminator) = body[current].terminator.as_mut() {
        *terminator = Terminator {
            source_info,
            kind: TerminatorKind::Goto { target: *pre_cond_bb.first().unwrap_or(&ffi_call_bb) },
        };
    }

    for (idx, &pre_bb) in pre_cond_bb.iter().enumerate() {
        let target = if let Some(val) = pre_cond_bb.get(idx + 1) { *val } else { ffi_call_bb };
        body[pre_bb].terminator_mut().successors_mut(|bb| *bb = target);
    }

    let first_post_bb = post_cond_bb[0];
    body[ffi_call_bb].terminator_mut().successors_mut(|bb| *bb = first_post_bb);

    for (idx, &post_bb) in post_cond_bb.iter().enumerate() {
        let target = if let Some(val) = post_cond_bb.get(idx + 1) { *val } else { target.unwrap() };
        body[post_bb].terminator_mut().successors_mut(|bb| *bb = target);
    }
}

fn instrument_drop<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>, current: BasicBlock) {
    let TerminatorKind::Drop { place, .. } = body[current].terminator().kind else {
        return;
    };

    let pre_cond_fn_name = "ffi_sanitizer_drop_pre_cond";
    let pre_cond_fn_symbol = Symbol::intern(pre_cond_fn_name);
    let Some(pre_cond_fn_def_id) = tcx.get_diagnostic_item(pre_cond_fn_symbol) else { return };
    let ty::RawPtr(generic_arg, _) = place.ty(&body.local_decls, tcx).ty.kind() else { return };

    let block = BasicBlockData::new(Some(body[current].terminator().clone()), false);
    let drop_bb = body.basic_blocks_mut().push(block);

    let source_info = body[current].terminator().source_info;
    let pre_cond_fn = Operand::function_handle(
        tcx,
        pre_cond_fn_def_id,
        [generic_arg.clone().into()],
        source_info.span,
    );

    let fn_arg = dummy_spanned(Operand::Copy(place));
    body[current].terminator_mut().kind = TerminatorKind::Call {
        func: pre_cond_fn,
        args: [fn_arg].into(),
        destination: get_temp_local(tcx, body),
        target: Some(drop_bb),
        unwind: UnwindAction::Continue,
        call_source: CallSource::Normal,
        fn_span: DUMMY_SP,
    };
}

fn instrument_func_can_be_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    for current in START_BLOCK..body.basic_blocks.next_index() {
        match body[current].terminator().kind {
            TerminatorKind::Return { .. } => instrument_return(tcx, body, current),
            _ => {}
        }
    }
}

fn instrument_main_func<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    let pre_cond_fn_name = "ffi_sanitizer_exit_pre_cond";
    let pre_cond_fn_symbol = Symbol::intern(pre_cond_fn_name);
    let Some(pre_cond_fn_def_id) = tcx.get_diagnostic_item(pre_cond_fn_symbol) else { return };

    for current in START_BLOCK..body.basic_blocks.next_index() {
        let TerminatorKind::Return {} = body[current].terminator().kind else {
            continue;
        };

        let block = BasicBlockData::new(Some(body[current].terminator().clone()), false);
        let ret_bb = body.basic_blocks_mut().push(block);

        let source_info = body[current].terminator().source_info;
        let pre_cond_fn = Operand::function_handle(tcx, pre_cond_fn_def_id, [], source_info.span);

        body[current].terminator_mut().kind = TerminatorKind::Call {
            func: pre_cond_fn,
            args: [].into(),
            destination: get_temp_local(tcx, body),
            target: Some(ret_bb),
            unwind: UnwindAction::Unreachable,
            call_source: CallSource::Misc,
            fn_span: DUMMY_SP,
        };
    }
}

struct DerefVisitor<'tcx> {
    vars: Vec<Place<'tcx>>,
}

impl<'tcx> Visitor<'tcx> for DerefVisitor<'tcx> {
    fn visit_place(&mut self, place: &Place<'tcx>, _: PlaceContext, _: Location) {
        let Some(p) = place.projection.get(0) else { return };
        if matches!(p, ProjectionElem::Deref) {
            let base_place = mir::Place { local: place.local, projection: ty::List::empty() };

            self.vars.push(base_place);
        }
    }
}
