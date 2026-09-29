//! Password recipes (ADR-0013).
//!
//! `vault__create` mints a unique random secret. The optional `rules`
//! argument uses Apple's Password Rules grammar, the format sites
//! publish in the `passwordrules` attribute, for example
//! `minlength: 8; maxlength: 16; required: lower, upper, digit;
//! allowed: [-_.]`.
//!
//! A recipe states what the site **forbids**. It never states strength.
//! The daemon mints the strongest secret the constraints permit — the
//! full allowed character set, and `maxlength` when the site gives one
//! — so a recipe cannot weaken a secret below what the site itself
//! allows, and no entropy floor is needed.

use rand::seq::{IndexedRandom, SliceRandom};

use crate::VaultError;

/// The recipe applied when a site publishes none: 32 characters over
/// the full alphanumeric set.
pub const DEFAULT_RECIPE: &str = "minlength: 32; maxlength: 32; allowed: upper, lower, digit";

const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const DIGIT: &str = "0123456789";
/// Apple's `special` class.
const SPECIAL: &str = "-~!@#$%^&*_+=`|(){}[:;\"'<>,.?]";
/// The longest secret we mint when a site sets no `maxlength`.
const UNBOUNDED_LENGTH: usize = 32;
/// A guard against a recipe that asks for an absurd length.
const MAX_LENGTH: usize = 128;

/// One parsed recipe: the character classes a site allows, the classes
/// it requires at least one of, and the length window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// Every character the site permits, deduplicated and ordered.
    pub allowed: Vec<char>,
    /// One character set per `required` class; the minted secret holds
    /// at least one character from each.
    pub required: Vec<Vec<char>>,
    pub min_length: usize,
    pub max_length: Option<usize>,
}

impl Recipe {
    /// The length to mint: `maxlength` when the site gives one, and the
    /// unbounded default otherwise, never below `minlength`.
    pub fn length(&self) -> usize {
        let wanted = self.max_length.unwrap_or(UNBOUNDED_LENGTH);
        wanted.max(self.min_length).min(MAX_LENGTH)
    }
}

/// Parse one Apple Password Rules string. An unknown property is
/// ignored, the way a browser ignores one it does not implement.
pub fn parse(rules: &str) -> Result<Recipe, VaultError> {
    let mut allowed: Vec<char> = Vec::new();
    let mut required: Vec<Vec<char>> = Vec::new();
    let mut min_length: usize = 1;
    let mut max_length: Option<usize> = None;
    let mut saw_allowed = false;

    for property in split_properties(rules) {
        let Some((key, value)) = property.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "minlength" => {
                min_length = value.parse().map_err(|_| {
                    VaultError::BadRecipe(format!("minlength {value:?} is not a number"))
                })?;
            }
            "maxlength" => {
                max_length = Some(value.parse().map_err(|_| {
                    VaultError::BadRecipe(format!("maxlength {value:?} is not a number"))
                })?);
            }
            "allowed" => {
                saw_allowed = true;
                for class in character_classes(value)? {
                    extend_unique(&mut allowed, &class);
                }
            }
            "required" => {
                // Each class a site names in `required` is its own
                // requirement: `required: lower, upper` means one of
                // each, not one of the two together.
                for class in character_classes(value)? {
                    extend_unique(&mut allowed, &class);
                    required.push(class);
                }
            }
            _ => {}
        }
    }

    // A site that names only `required` classes allows those classes.
    // A site that names neither gets the default alphabet.
    if !saw_allowed && required.is_empty() {
        extend_unique(&mut allowed, &default_alphabet());
    }
    if allowed.is_empty() {
        return Err(VaultError::BadRecipe(
            "the recipe allows no characters".to_string(),
        ));
    }
    if min_length == 0 {
        min_length = 1;
    }
    if let Some(max) = max_length
        && max < min_length
    {
        return Err(VaultError::BadRecipe(format!(
            "maxlength {max} is below minlength {min_length}"
        )));
    }
    if min_length > MAX_LENGTH {
        return Err(VaultError::BadRecipe(format!(
            "minlength {min_length} is above the {MAX_LENGTH} character limit"
        )));
    }
    if required.len() > max_length.unwrap_or(UNBOUNDED_LENGTH).max(min_length) {
        return Err(VaultError::BadRecipe(
            "the recipe requires more classes than it has characters".to_string(),
        ));
    }
    Ok(Recipe {
        allowed,
        required,
        min_length,
        max_length,
    })
}

/// Mint one secret for a recipe: every character drawn uniformly from
/// the allowed set, then one character per required class placed at a
/// random position, then shuffled.
pub fn mint(recipe: &Recipe) -> String {
    let mut rng = rand::rng();
    let length = recipe.length();
    let mut chars: Vec<char> = (0..length)
        .map(|_| {
            *recipe
                .allowed
                .choose(&mut rng)
                .expect("allowed is not empty")
        })
        .collect();
    // Placing the required characters at distinct random positions
    // keeps every other position uniform over the full alphabet.
    let mut positions: Vec<usize> = (0..length).collect();
    positions.shuffle(&mut rng);
    for (class, position) in recipe.required.iter().zip(positions) {
        if let Some(pick) = class.choose(&mut rng) {
            chars[position] = *pick;
        }
    }
    chars.into_iter().collect()
}

/// Mint a secret for a rules string, returning the secret and the
/// recipe as it was applied, which the record stores.
pub fn mint_for(rules: Option<&str>) -> Result<(String, String), VaultError> {
    let applied = rules.map(str::trim).filter(|rules| !rules.is_empty());
    let stored = applied.unwrap_or(DEFAULT_RECIPE).to_string();
    let recipe = parse(&stored)?;
    Ok((mint(&recipe), stored))
}

fn split_properties(rules: &str) -> Vec<String> {
    // Semicolons inside a `[...]` character list are literal.
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_set = false;
    for c in rules.chars() {
        match c {
            '[' => {
                in_set = true;
                current.push(c);
            }
            ']' => {
                in_set = false;
                current.push(c);
            }
            ';' if !in_set => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
}

/// The character classes one `allowed`/`required` value names, one
/// entry per named class: the standard keywords, plus `[...]` literal
/// sets.
fn character_classes(value: &str) -> Result<Vec<Vec<char>>, VaultError> {
    let mut classes = Vec::new();
    for token in split_classes(value) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if let Some(set) = token
            .strip_prefix('[')
            .and_then(|set| set.strip_suffix(']'))
        {
            let mut literal = Vec::new();
            extend_unique(&mut literal, &set.chars().collect::<Vec<char>>());
            if !literal.is_empty() {
                classes.push(literal);
            }
            continue;
        }
        let class: Vec<char> = match token.to_ascii_lowercase().as_str() {
            "upper" => UPPER.chars().collect(),
            "lower" => LOWER.chars().collect(),
            "digit" | "digits" => DIGIT.chars().collect(),
            "special" => SPECIAL.chars().collect(),
            "ascii-printable" => (0x20u8..0x7f).map(char::from).collect(),
            "unicode" => default_alphabet(),
            other => {
                return Err(VaultError::BadRecipe(format!(
                    "unknown character class {other:?}"
                )));
            }
        };
        classes.push(class);
    }
    Ok(classes)
}

/// Split a class list on commas outside `[...]`.
fn split_classes(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_set = false;
    for c in value.chars() {
        match c {
            '[' => {
                in_set = true;
                current.push(c);
            }
            ']' => {
                in_set = false;
                current.push(c);
            }
            ',' if !in_set => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);
    parts
}

fn default_alphabet() -> Vec<char> {
    UPPER
        .chars()
        .chain(LOWER.chars())
        .chain(DIGIT.chars())
        .collect()
}

fn extend_unique(target: &mut Vec<char>, source: &[char]) {
    for c in source {
        if !target.contains(c) {
            target.push(*c);
        }
    }
}

/// True when the secret satisfies every constraint of the recipe. The
/// mint path is exercised through this in tests.
pub fn satisfies(secret: &str, recipe: &Recipe) -> bool {
    let chars: Vec<char> = secret.chars().collect();
    chars.len() >= recipe.min_length
        && recipe.max_length.is_none_or(|max| chars.len() <= max)
        && chars.iter().all(|c| recipe.allowed.contains(c))
        && recipe
            .required
            .iter()
            .all(|class| chars.iter().any(|c| class.contains(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_recipe_is_thirty_two_alphanumerics() {
        let (secret, stored) = mint_for(None).unwrap();

        assert_eq!(stored, DEFAULT_RECIPE);
        assert_eq!(secret.chars().count(), 32);
        assert!(secret.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn a_recipe_mints_the_strongest_secret_it_permits() {
        // The site forbids more than eight characters and demands a
        // digit; the daemon uses all eight and the full alphabet.
        let (secret, stored) = mint_for(Some("maxlength: 8; required: digit")).unwrap();

        assert_eq!(stored, "maxlength: 8; required: digit");
        assert_eq!(secret.chars().count(), 8);
        assert!(secret.chars().any(|c| c.is_ascii_digit()), "{secret}");
        let recipe = parse(&stored).unwrap();
        assert!(satisfies(&secret, &recipe), "{secret}");
    }

    #[test]
    fn an_explicit_allowed_set_bounds_the_alphabet() {
        let rules = "minlength: 8; maxlength: 16; required: lower, upper, digit; allowed: [-_.]";
        let recipe = parse(rules).unwrap();

        assert_eq!(recipe.min_length, 8);
        assert_eq!(recipe.max_length, Some(16));
        assert_eq!(recipe.required.len(), 3);
        assert_eq!(recipe.length(), 16);

        for _ in 0..64 {
            let secret = mint(&recipe);
            assert!(satisfies(&secret, &recipe), "{secret}");
            assert!(
                secret
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                "{secret}"
            );
        }
    }

    #[test]
    fn a_semicolon_inside_a_character_set_is_literal() {
        let recipe = parse("allowed: [;:]; minlength: 4").unwrap();

        assert_eq!(recipe.allowed, vec![';', ':']);
        assert_eq!(recipe.min_length, 4);
    }

    #[test]
    fn an_unusable_recipe_is_refused() {
        assert!(parse("allowed: ").is_err());
        assert!(parse("allowed: martian").is_err());
        assert!(parse("minlength: 10; maxlength: 4; allowed: digit").is_err());
        assert!(parse("minlength: eight; allowed: digit").is_err());
        assert!(parse("maxlength: 2; required: upper, lower, digit").is_err());
    }

    #[test]
    fn two_mints_of_one_recipe_differ() {
        let recipe = parse(DEFAULT_RECIPE).unwrap();
        assert_ne!(mint(&recipe), mint(&recipe));
    }
}
