use std::sync::Arc;

use crate::cli::StrategyArg;
use crate::reorganize::llm::ClaudeCliClient;
use crate::reorganize::{
    GroupByFile, HierarchicalReorganizer, LlmReorganizer, PreserveOriginal, Reorganizer, Squash,
};

/// Factory object responsible for instantiating reorganizers selected by the CLI.
#[derive(Clone, Copy, Default)]
pub struct StrategyFactory;

impl StrategyFactory {
    pub fn new() -> Self {
        Self
    }

    pub fn create(&self, strategy: StrategyArg) -> Box<dyn Reorganizer> {
        match strategy {
            StrategyArg::Preserve => Box::new(PreserveOriginal),
            StrategyArg::ByFile => Box::new(GroupByFile),
            StrategyArg::Squash => Box::new(Squash),
            StrategyArg::Llm => Box::new(LlmReorganizer::new(Box::new(ClaudeCliClient::new()))),
            StrategyArg::Hierarchical => {
                let client = Arc::new(ClaudeCliClient::new());
                Box::new(HierarchicalReorganizer::new(Some(client)))
            }
        }
    }
}
