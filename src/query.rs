// https://crates.io/crates/salsa
// hello world https://github.com/salsa-rs/salsa/blob/master/examples/hello_world/main.rs

use crate::error::ChalkError;
use crate::lowering::LowerProgram;
use crate::program::Program;
use crate::program_environment::ProgramEnvironment;
use chalk_ir::tls;
use chalk_ir::TraitId;
use chalk_rules::coherence::orphan;
use chalk_rules::coherence::{CoherenceSolver, SpecializationPriorities};
use chalk_rules::wf;
use chalk_solve::ProgramClauseSet;
use chalk_solve::SolverChoice;
use std::collections::BTreeMap;
use std::sync::Arc;

#[salsa::query_group(Lowering)]
pub trait LoweringDatabase: ProgramClauseSet {
    #[salsa::input]
    fn program_text(&self) -> Arc<String>;

    #[salsa::input]
    fn solver_choice(&self) -> SolverChoice;

    fn program_ir(&self) -> Result<Arc<Program>, ChalkError>;

    /// Performs coherence check and computes which impls specialize
    /// one another (the "specialization priorities").
    fn coherence(&self) -> Result<BTreeMap<TraitId, Arc<SpecializationPriorities>>, ChalkError>;

    fn orphan_check(&self) -> Result<(), ChalkError>;

    /// The lowered IR, with coherence, orphan, and WF checks performed.
    fn checked_program(&self) -> Result<Arc<Program>, ChalkError>;

    /// The program as logic.
    fn environment(&self) -> Result<Arc<ProgramEnvironment>, ChalkError>;
}

fn program_ir(db: &impl LoweringDatabase) -> Result<Arc<Program>, ChalkError> {
    let text = db.program_text();
    Ok(Arc::new(chalk_parse::parse_program(&text)?.lower()?))
}

fn orphan_check(db: &impl LoweringDatabase) -> Result<(), ChalkError> {
    let program = db.program_ir()?;
    let solver_choice = db.solver_choice();

    tls::set_current_program(&program, || -> Result<(), ChalkError> {
        let local_impls = program.local_impl_ids();
        for impl_id in local_impls {
            orphan::perform_orphan_check(&*program, db, solver_choice, impl_id)?;
        }
        Ok(())
    })
}

fn coherence(
    db: &impl LoweringDatabase,
) -> Result<BTreeMap<TraitId, Arc<SpecializationPriorities>>, ChalkError> {
    let program = db.program_ir()?;

    let priorities_map: Result<BTreeMap<_, _>, ChalkError> = program
        .trait_data
        .keys()
        .map(|&trait_id| {
            let solver = CoherenceSolver::new(&*program, db, db.solver_choice(), trait_id);
            let priorities = solver.specialization_priorities()?;
            Ok((trait_id, priorities))
        })
        .collect();
    let priorities_map = priorities_map?;

    let () = db.orphan_check()?;

    Ok(priorities_map)
}

fn checked_program(db: &impl LoweringDatabase) -> Result<Arc<Program>, ChalkError> {
    let program = db.program_ir()?;

    db.coherence()?;

    let () = tls::set_current_program(&program, || -> Result<(), ChalkError> {
        let solver = wf::WfSolver {
            program: &*program,
            env: db,
            solver_choice: db.solver_choice(),
        };

        for &id in program.struct_data.keys() {
            solver.verify_struct_decl(id)?;
        }

        for &impl_id in program.impl_data.keys() {
            solver.verify_trait_impl(impl_id)?;
        }

        Ok(())
    })?;

    Ok(program)
}

fn environment(db: &impl LoweringDatabase) -> Result<Arc<ProgramEnvironment>, ChalkError> {
    let env = db.program_ir()?.environment();
    Ok(Arc::new(env))
}
