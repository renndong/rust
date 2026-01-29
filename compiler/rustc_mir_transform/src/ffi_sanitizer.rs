#![allow(dead_code)]
#![allow(warnings)]

use rustc_abi::ExternAbi;
use rustc_hir as hir;
use rustc_hir::Attribute;
use rustc_hir::attrs::AttributeKind;
use rustc_middle::mir::interpret::Scalar;
use rustc_middle::mir::*;
use rustc_middle::ty;
use rustc_middle::ty::TyCtxt;
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
            instrument_func_with_ffi_call(tcx, body);
        } else if can_be_ffi_call(tcx, body) {
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

fn instrument_func_with_ffi_call<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    for current in START_BLOCK..body.basic_blocks.next_index() {
        match body[current].terminator().kind {
            TerminatorKind::Call { .. } => instrument_call(tcx, body, current),
            TerminatorKind::Return { .. } => instrument_return(tcx, body, current),
            TerminatorKind::TailCall { .. } => instrument_ptr_use_term(tcx, body, current),
            TerminatorKind::Drop { .. } => instrument_drop(tcx, body, current), // may never execute
            _ => {}
        }
    }
}

fn select_raw_ptr_args<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut Body<'tcx>,
    args: &Box<[Spanned<Operand<'tcx>>]>,
) -> Vec<(ty::Ty<'tcx>, Spanned<Operand<'tcx>>)> {
    let mut func_args = Vec::new();
    for arg in args.iter() {
        match &arg.node {
            Operand::Copy(place) | Operand::Move(place) => {
                if let ty::RawPtr(pty, _) = place.ty(&body.local_decls, tcx).ty.kind() {
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
    let ret_place = Place::from(RETURN_PLACE);

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
        destination: Place::return_place(),
        target: Some(ret_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
}

fn instrument_ptr_use_term<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>, current: BasicBlock) {}

fn instrument_call<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>, current: BasicBlock) {
    let TerminatorKind::Call { func, args, destination, target, unwind, call_source, fn_span } =
        body[current].terminator().kind.clone()
    else {
        return;
    };
    if !is_ffi_call(tcx, body, &func) {
        return instrument_ptr_use_term(tcx, body, current);
    }

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
                destination: Place::return_place(),
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
                destination: Place::return_place(),
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
    let TerminatorKind::Drop { place, target, unwind, replace, drop, async_fut } =
        body[current].terminator().kind
    else {
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
        destination: Place::return_place(),
        target: Some(drop_bb),
        unwind,
        call_source: CallSource::Normal,
        fn_span: DUMMY_SP,
    };
}

fn find_function_by_name<'tcx>(
    tcx: TyCtxt<'tcx>,
    name: Symbol,
) -> Option<rustc_hir::def_id::DefId> {
    for local_def_id in tcx.hir_body_owners() {
        // body_owners() 获取定义了 HIR Body 的 DefIds
        let def_kind = tcx.def_kind(local_def_id.to_def_id());

        if matches!(def_kind, hir::def::DefKind::Fn | hir::def::DefKind::AssocFn) {
            let def_path = tcx.def_path(local_def_id.to_def_id());
            // 检查路径的最后一部分是否匹配名称
            if def_path.data.last().map_or(false, |data| data.data.get_opt_name() == Some(name)) {
                eprintln!(
                    "Found function {:?} at {:?}",
                    name,
                    tcx.def_path_str(local_def_id.to_def_id())
                );
                return Some(local_def_id.to_def_id());
            }
        }
    }
    None
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
            destination: Place::return_place(),
            target: Some(ret_bb),
            unwind: UnwindAction::Unreachable,
            call_source: CallSource::Misc,
            fn_span: DUMMY_SP,
        };
    }
}
