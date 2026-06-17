//! Message quality criterion: measures how well the commit message communicates intent.
//!
//! The diff already says *what* changed. The message exists to say *why*: what was
//! broken, what pressure created this, what would happen if we didn't make the change.
//!
//! The title should name the system, the decision, and the scope in ~50 chars — a
//! teammate should get it from `git log --oneline` alone. The body should give a
//! stranger arriving via `git blame` enough context to reconstruct the author's
//! reasoning: problem → approach → trade-offs.
//!
//! A specific failure mode this criterion targets: bodies that *reinvent the diff
//! in prose* by enumerating added identifiers — tag names, field names, function
//! names, histogram keys, file paths. That's not motivation, it's an inventory
//! the diff already provides. The body should explain the gap that motivated the
//! change ("we had X but couldn't tell Y") and stop.

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
            The title is a single line, ≤50 chars, naming the system, the action, and \
            the reason so a teammate scanning `git log --oneline` understands it and \
            could find it with `git log --grep`. Never a multi-sentence summary, never \
            a paragraph. The body is short and motivation-first: what gap or pressure \
            led to the change, and what's now possible that wasn't before. Not a diff \
            walkthrough, not an inventory of added identifiers (tag names, field names, \
            function names, histogram keys, file paths) — those are visible in the diff. \
            A stranger via `git blame` should be able to reconstruct the author's \
            reasoning in under a minute."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 1.2, "Missing or meaningless — no signal at all")
                .with_indicators(vec![
                    "Single word like 'fix', 'update', 'changes', 'wip'".to_string(),
                    "Message is empty or purely mechanical (e.g. 'merge branch')".to_string(),
                    "Could not find this commit with `git log --grep` for any relevant term".to_string(),
                    "A stranger via git blame learns nothing".to_string(),
                ]),
            AssessmentLevel::new(2, 1.2, "Malformed title, or body inventories the diff — says what, not why")
                .with_indicators(vec![
                    "Title is multi-sentence, wraps lines, or runs well past 50 chars".to_string(),
                    "Title names the action but not the system or reason ('Increase threads')".to_string(),
                    "Body enumerates added identifiers (tag/field/function/file/histogram names) — reinvents the diff in prose".to_string(),
                    "Body describes control flow, parameter lists, or what each helper does — info already in the diff".to_string(),
                    "Fails the grep test: a teammate could not find this with a reasonable search term".to_string(),
                    "No context a future reader couldn't get from the diff itself".to_string(),
                ]),
            AssessmentLevel::new(3, 1.2, "Gestures at motivation but leans on implementation detail to fill space")
                .with_indicators(vec![
                    "Body has *some* motivation, but most of the prose enumerates what was added".to_string(),
                    "Motivation is forward-looking ('so we can…') without naming the concrete gap that existed before".to_string(),
                    "Body is padded — repeats the title, lists what got registered/wrapped, or wanders".to_string(),
                    "A teammate scanning the body gets the structure of the change but not the decision".to_string(),
                ]),
            AssessmentLevel::new(4, 1.2, "Body leads with the gap; doesn't enumerate the diff")
                .with_indicators(vec![
                    "Title is ≤50 chars and packs system, action, and scope — passes the grep test".to_string(),
                    "Body opens by naming the concrete gap or pressure ('we had aggregate timing but couldn't attribute slow calls to specific blocks')".to_string(),
                    "Approach is described at a strategy level, not by listing added identifiers".to_string(),
                    "A stranger via `git blame` could understand the reasoning without the diff".to_string(),
                    "Mentions what's now possible that wasn't before, or what would have happened without this change".to_string(),
                ]),
            AssessmentLevel::new(5, 1.2, "Complete context, tightly written — every line earns its place")
                .with_indicators(vec![
                    "Title is ≤50 chars, names system + symptom/decision + scope, immediately findable via grep".to_string(),
                    "Body answers in order: what was wrong/missing, what's now possible, any non-obvious trade-off — and stops".to_string(),
                    "Approach described at the level of strategy or constraint, never by listing identifiers from the diff".to_string(),
                    "Discusses alternatives considered or why this approach over others, in a sentence or two, where non-obvious".to_string(),
                    "Links to issues, flakes, or prior context where relevant".to_string(),
                    "Cutting any sentence would lose meaningful context — nothing extraneous, no inventory".to_string(),
                ]),
        ],
    }
}

/// Returns the Level 5 (ideal) message quality guidance, formatted as a prompt
/// fragment for use in commit message generation prompts.
///
/// This reads directly from the criterion definition so there's a single source
/// of truth for what a good commit message looks like, then appends an explicit
/// DO/DON'T list naming the failure mode the criterion penalises (bodies that
/// reinvent the diff in prose by listing added identifiers).
pub fn guidelines() -> String {
    let def = definition();
    let top = &def.levels[4];
    let mut out = format!("{}\n", top.description);
    for indicator in &top.indicators {
        out.push_str(&format!("- {}\n", indicator));
    }

    out.push_str(
        "\n\
        **The body must explain WHY, not WHAT.** The diff already says what changed. \
        A good body answers, in order: (1) what gap or pressure existed before this \
        change, (2) what the change makes possible that wasn't possible before, \
        (3) any non-obvious trade-off — then stops.\n\
        \n\
        DO:\n\
        - Lead with the concrete prior gap. (\"We had aggregate timing for the whole \
          call but couldn't tell which sub-step was slow.\")\n\
        - Describe the approach at the level of *strategy*, not identifiers.\n\
        - Make every sentence load-bearing. If deleting it loses no context, delete it.\n\
        \n\
        DON'T:\n\
        - Enumerate added identifiers — tag names, field names, function names, file \
          names, histogram keys, config values. Those are visible in the diff and \
          listing them is reinventing the diff in prose.\n\
        - Describe control flow, parameter lists, or what each helper does step-by-step.\n\
        - Use only forward-looking motivation (\"so we can…\") without naming the \
          concrete prior gap. \"So we can attribute slow calls\" is weak; \"we had \
          aggregate timing but no per-sub-step attribution\" is what's needed.\n\
        - Restate the title in the body's first sentence.\n",
    );

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
