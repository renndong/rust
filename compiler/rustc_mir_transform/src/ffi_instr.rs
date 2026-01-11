use rustc_middle::mir::*;
use rustc_middle::ty;
use rustc_middle::ty::TyCtxt;

pub(super) struct FFIInstr;

impl<'tcx> crate::MirPass<'tcx> for FFIInstr {
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        if !should_instrument(tcx, body) {
            println!("Skipping FFIInstr");
            return;
        }

        println!("FFIInstr");
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
