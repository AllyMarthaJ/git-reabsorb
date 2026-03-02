//! Message quality criterion: measures how well the commit message communicates intent.
//!
//! The diff already says *what* changed. The message exists to say *why*: what was
//! broken, what pressure created this, what would happen if we didn't make the change.
//!
//! The title should name the system, the decision, and the scope in ~50 chars — a
//! teammate should get it from `git log --oneline` alone. The body should give a
//! stranger arriving via `git blame` enough context to reconstruct the author's
//! reasoning: problem → approach → trade-offs.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the message quality criterion definition.
///
/// This criterion has a higher weight (1.2) because commit messages are
/// critical for long-term code maintenance and understanding.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::MessageQuality,
        description: "The diff already says what changed — the message must say why. \
            The title names the system, the action, and the reason in ~50 chars so a \
            teammate scanning `git log --oneline` understands the line and could find it \
            with `git log --grep`. The body provides context for a stranger arriving via \
            `git blame`: problem → approach → trade-offs. Not a diff walkthrough — the \
            reader should be able to reconstruct the author's reasoning, not their keystrokes."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 1.2, "Missing or meaningless — no signal at all")
                .with_indicators(vec![
                    "Single word like 'fix', 'update', 'changes', 'wip'".to_string(),
                    "Message is empty or purely mechanical (e.g. 'merge branch')".to_string(),
                    "Could not find this commit with `git log --grep` for any relevant term".to_string(),
                    "A stranger via git blame learns nothing".to_string(),
                ]),
            AssessmentLevel::new(2, 1.2, "Restates the diff — says what, not why")
                .with_indicators(vec![
                    "Title names the action but not the system or reason ('Increase threads')".to_string(),
                    "Body walks through the diff in prose instead of explaining motivation".to_string(),
                    "Fails the grep test: a teammate could not find this with a reasonable search term".to_string(),
                    "No context a future reader couldn't get from the diff itself".to_string(),
                ]),
            AssessmentLevel::new(3, 1.2, "Names the system and action, but motivation is thin")
                .with_indicators(vec![
                    "Title identifies the area and what was done, but not why".to_string(),
                    "Some body context, but doesn't explain what was broken or what pressure led here".to_string(),
                    "A teammate scanning `git log --oneline` gets the gist but not the decision".to_string(),
                    "Removing the body would leave the title still roughly orienting".to_string(),
                ]),
            AssessmentLevel::new(4, 1.2, "Names system + decision; body explains reasoning")
                .with_indicators(vec![
                    "Title packs system, action, and scope into ~50 chars — passes the grep test".to_string(),
                    "Body explains what was broken or what pressure created this change".to_string(),
                    "A stranger via `git blame` could understand the reasoning without the diff".to_string(),
                    "Mentions implications or what would happen if we didn't make this change".to_string(),
                ]),
            AssessmentLevel::new(5, 1.2, "Complete context — someone could reimplement from the message alone")
                .with_indicators(vec![
                    "Title names system, symptom/decision, and scope — immediately findable via grep".to_string(),
                    "Body follows problem → approach → trade-offs structure".to_string(),
                    "Discusses alternatives considered or why this approach over others".to_string(),
                    "Links to issues, flakes, or prior context where relevant".to_string(),
                    "An agent could reimplement the change from the message alone".to_string(),
                ]),
        ],
    }
}

/// Returns the Level 5 (ideal) message quality guidance, formatted as a prompt
/// fragment for use in commit message generation prompts.
///
/// This reads directly from the criterion definition so there's a single source
/// of truth for what a good commit message looks like.
pub fn guidelines() -> String {
    let def = definition();
    let top = &def.levels[4];
    let mut out = format!("{}\n", top.description);
    for indicator in &top.indicators {
        out.push_str(&format!("- {}\n", indicator));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_higher_weight() {
        let def = definition();
        assert_eq!(def.levels[0].weight, 1.2);
    }

    #[test]
    fn max_weighted_score() {
        let def = definition();
        assert_eq!(def.max_weighted_score(), 6.0); // 5 * 1.2
    }
}
