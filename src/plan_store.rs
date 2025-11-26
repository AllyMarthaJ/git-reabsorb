pub use crate::reorganize::plan_file::{PlanFileError, SavedPlan};

use crate::reorganize::plan_file;

/// Trait abstracting how plans are persisted between runs.
pub trait PlanStore {
    fn load(&self) -> Result<SavedPlan, PlanFileError>;
    fn save(&self, plan: &SavedPlan) -> Result<(), PlanFileError>;
    fn delete(&self) -> Result<(), PlanFileError>;
    fn exists(&self) -> bool;
}

/// Filesystem-backed plan store using `.git/scramble/plan.json`.
pub struct FilePlanStore;

impl FilePlanStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FilePlanStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanStore for FilePlanStore {
    fn load(&self) -> Result<SavedPlan, PlanFileError> {
        plan_file::load_plan()
    }

    fn save(&self, plan: &SavedPlan) -> Result<(), PlanFileError> {
        plan_file::save_plan(plan).map(|_| ())
    }

    fn delete(&self) -> Result<(), PlanFileError> {
        plan_file::delete_plan()
    }

    fn exists(&self) -> bool {
        plan_file::has_saved_plan()
    }
}
