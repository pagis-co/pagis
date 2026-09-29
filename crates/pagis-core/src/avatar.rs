//! Saved sprite choices. The UI and daemon use the same asset catalog.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AvatarAppearance {
    pub sprite: String,
    pub preset: String,
    pub colors: BTreeMap<String, String>,
    pub accessories: BTreeMap<String, bool>,
}

#[derive(Deserialize)]
struct SpriteDefinition {
    presets: BTreeMap<String, serde_json::Value>,
    colors: BTreeMap<String, serde_json::Value>,
    #[serde(rename = "accessoryLabels")]
    accessories: BTreeMap<String, String>,
}
static CATALOG: LazyLock<BTreeMap<String, SpriteDefinition>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../assets/avatars/catalog.json"))
        .expect("the checked sprite catalog is valid")
});

impl Default for AvatarAppearance {
    fn default() -> Self {
        Self {
            sprite: "pixie".into(),
            preset: "mint".into(),
            colors: BTreeMap::new(),
            accessories: BTreeMap::new(),
        }
    }
}
impl AvatarAppearance {
    pub fn validate(&self) -> Result<(), String> {
        let sprite = CATALOG
            .get(&self.sprite)
            .ok_or("Choose a sprite from the catalog.")?;
        if !sprite.presets.contains_key(&self.preset) {
            return Err("Choose a style for this sprite.".into());
        }
        for (slot, color) in &self.colors {
            if !sprite.colors.contains_key(slot) {
                return Err("This sprite does not have that color control.".into());
            }
            if color.len() != 7
                || !color.starts_with('#')
                || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err("Use a six-digit hex color, such as #AABBCC.".into());
            }
        }
        if self
            .accessories
            .keys()
            .any(|key| !sprite.accessories.contains_key(key))
        {
            return Err("This sprite does not have that accessory.".into());
        }
        Ok(())
    }
}
