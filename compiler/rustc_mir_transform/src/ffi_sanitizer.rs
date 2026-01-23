use rustc_middle::mir::*;
use rustc_middle::ty;
use rustc_middle::ty::TyCtxt;

pub(super) struct FFISanitizer;

impl<'tcx> crate::MirPass<'tcx> for FFISanitizer {
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        if !tcx.features().ffi_san() {
            return;
        }
        if !should_instrument(tcx, body) {
            return;
        }
        instrument(tcx, body);
    }

    fn is_required(&self) -> bool {
        true
    }
}

fn should_instrument<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>) -> bool {
    let mut has_ffi = false;
    for (bb, block) in body.basic_blocks.iter_enumerated() {
        let Some(terminator) = &block.terminator else { continue };

        if let TerminatorKind::Call { func, .. } = &terminator.kind {
            if let ty::FnDef(def_id, _) = func.ty(body, tcx).kind() {
                let def_path = tcx.def_path(*def_id).to_string_no_crate_verbose();
                if tcx.is_foreign_item(*def_id) {
                    println!("Found FFI call in {:?}, name: {:?}", bb, def_path);
                    has_ffi = true;
                    break;
                }
            }
        }
    }
    return has_ffi;
}

fn instrument<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    _ = tcx;
    _ = body;

    
}