#![allow(dead_code)]

use rustc_hir as hir;
use rustc_middle::mir::*;
use rustc_middle::ty;
use rustc_middle::ty::TyCtxt;
use rustc_span::symbol::Symbol;

pub(super) struct FFISanitizer;

impl<'tcx> crate::MirPass<'tcx> for FFISanitizer {
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        if !tcx.features().ffi_sanitizer() || !should_instrument(tcx, body) {
            return;
        }
        instrument(tcx, body);
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

fn should_instrument<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>) -> bool {
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

fn instrument<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    let name = "ffi_sanitizer_non_null";
    let symbol_name = Symbol::intern(name);

    let Some(san_fn_def_id) = tcx.get_diagnostic_item(symbol_name) else { return };

    for bb in START_BLOCK..body.basic_blocks.next_index() {
        let TerminatorKind::Call { func, args, destination, target, unwind, call_source, fn_span } =
            body[bb].terminator().kind.clone()
        else {
            continue;
        };

        if !is_ffi_call(tcx, body, &func) {
            continue;
        }

        let source_info = body[bb].terminator().source_info;
        let second_terminator = Terminator {
            source_info,
            kind: TerminatorKind::Call {
                func: func.clone(),
                args: args.clone(),
                destination,
                target,
                unwind,
                call_source,
                fn_span,
            },
        };

        let mut san_func_args = Vec::new();
        for arg in args.iter() {
            match &arg.node {
                Operand::Copy(place) => {
                    if let ty::RawPtr(pty, _) = place.ty(&body.local_decls, tcx).ty.kind() {
                        san_func_args.push((pty.clone(), arg.clone()));
                    }
                }
                Operand::Move(_) => {
                    println!("move");
                }
                Operand::Constant(_) => {
                    println!("const");
                }
            }
        }

        let Some((generic_arg, san_arg)) = san_func_args.pop() else {
            println!("generic_arg is none");
            continue;
        };

        let second_block = BasicBlockData::new(Some(second_terminator), false);
        let second_idx = body.basic_blocks_mut().push(second_block);

        let san_func =
            Operand::function_handle(tcx, san_fn_def_id, [generic_arg.into()], source_info.span);

        let first_terminator = TerminatorKind::Call {
            func: san_func,
            args: [san_arg.into()].into(),
            destination: Place::return_place(),
            target: Some(second_idx),
            unwind,
            call_source,
            fn_span,
        };
        let terminator = body[bb].terminator.as_mut().expect("invalid terminator");
        terminator.kind = first_terminator;
    }
    // body.basic_blocks = BasicBlocks::new(blocks);
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
                eprintln!("Found function {:?} at {:?}", name, tcx.def_path_str(local_def_id.to_def_id()));
                return Some(local_def_id.to_def_id());
            }
        }
    }
    None
}
