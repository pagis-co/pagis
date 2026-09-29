//! The Skills section of the system prompt (ADR-0017).
//!
//! The section names every Skill the agent holds and what each one is
//! for. The body stays out: `skill_load` brings it in when the agent
//! decides a Skill applies.

use pagis_core::Skill;

/// How many Skills the section lists. Over this many, the first ones
/// by install order are listed and a line says how many more exist
/// (ADR-0017).
pub const MAX_LISTED_SKILLS: usize = 40;

/// The Skills section, as the prompt carries it.
pub fn skills_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return "Skills: you hold no skills. A skill arrives when the user installs a plugin \
                and grants it to you."
            .to_string();
    }
    let mut section = String::from(
        "Skills: each line below is one instruction document. Read the one that covers \
         the work with skill_load before you start that work; its own files are in your \
         computer beside it. You hold:",
    );
    for skill in skills.iter().take(MAX_LISTED_SKILLS) {
        section.push_str(&format!("\n- {}: {}", skill.qualified(), skill.description));
    }
    let rest = skills.len().saturating_sub(MAX_LISTED_SKILLS);
    if rest == 1 {
        section.push_str("\n\n1 more skill is installed but not listed here.");
    } else if rest > 1 {
        section.push_str(&format!(
            "\n\n{rest} more skills are installed but not listed here."
        ));
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(index: usize) -> Skill {
        Skill {
            plugin: "acme".to_string(),
            name: format!("skill{index}"),
            description: format!("Does thing {index}."),
        }
    }

    #[test]
    fn an_agent_with_no_skills_is_told_how_one_arrives() {
        let section = skills_section(&[]);
        assert!(section.contains("you hold no skills"));
        assert!(section.contains("grants it to you"));
    }

    #[test]
    fn the_section_names_every_skill_by_plugin_and_skill() {
        let section = skills_section(&[skill(1), skill(2)]);
        assert!(section.contains("- acme:skill1: Does thing 1."));
        assert!(section.contains("- acme:skill2: Does thing 2."));
        assert!(!section.contains("not listed here"));
    }

    #[test]
    fn over_the_cap_the_first_forty_are_listed_with_a_count_of_the_rest() {
        let skills: Vec<Skill> = (1..=45).map(skill).collect();
        let section = skills_section(&skills);
        assert!(section.contains("- acme:skill40:"));
        assert!(!section.contains("- acme:skill41:"));
        assert!(section.contains("5 more skills are installed but not listed here."));
    }

    #[test]
    fn one_skill_over_the_cap_reads_as_one() {
        let skills: Vec<Skill> = (1..=41).map(skill).collect();
        assert!(skills_section(&skills).contains("1 more skill is installed but not listed here."));
    }
}
