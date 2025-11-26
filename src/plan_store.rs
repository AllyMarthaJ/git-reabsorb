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
pub struct FilePlanStore {
    namespace: String,
}

impl FilePlanStore {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
        }
    }
}

impl Default for FilePlanStore {
    fn default() -> Self {
        Self::new("default")
    }
}

impl PlanStore for FilePlanStore {
    fn load(&self) -> Result<SavedPlan, PlanFileError> {
        plan_file::load_plan(&self.namespace)
    }

    fn save(&self, plan: &SavedPlan) -> Result<(), PlanFileError> {
        plan_file::save_plan(&self.namespace, plan).map(|_| ())
    }

    fn delete(&self) -> Result<(), PlanFileError> {
        plan_file::delete_plan(&self.namespace)
    }

    fn exists(&self) -> bool {
        plan_file::has_saved_plan(&self.namespace)
    }
}
