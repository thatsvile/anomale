use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::map::Palette;
use crate::recipe::Recipe;
use crate::{list_recipe_names, palette_from_image};
use vile_protocol::{palettes_dir, sanitize_recipe_name};

pub struct RecipeRow {
    pub name: String,
    pub palette: Result<Palette, String>,
}

pub fn load_rows(image: &Path) -> Vec<RecipeRow> {
    let Ok(names) = list_recipe_names() else {
        return Vec::new();
    };
    names
        .into_iter()
        .map(|name| {
            let palette = load_named(&name)
                .and_then(|recipe| palette_from_image(&recipe, image))
                .map_err(|err| err.to_string());
            RecipeRow { name, palette }
        })
        .collect()
}

pub fn load_named(name: &str) -> anyhow::Result<Recipe> {
    let name = sanitize_recipe_name(name);
    if name.is_empty() {
        anyhow::bail!("invalid recipe name");
    }
    Recipe::load(&recipe_path(&name))
}

pub fn recipe_path(name: &str) -> PathBuf {
    palettes_dir().join(format!("{name}.json"))
}

pub fn delete_named(name: &str) -> anyhow::Result<()> {
    let name = sanitize_recipe_name(name);
    if name.is_empty() {
        anyhow::bail!("invalid recipe name");
    }
    let path = recipe_path(&name);
    fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Recipe;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct RestoreEnv {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn delete_named_roundtrip() {
        let _lock = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "vile-palettes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config_home = root.join("config");
        fs::create_dir_all(&config_home).unwrap();
        let _restore = RestoreEnv {
            key: "XDG_CONFIG_HOME",
            old: std::env::var_os("XDG_CONFIG_HOME"),
        };
        std::env::set_var("XDG_CONFIG_HOME", &config_home);

        let recipe = Recipe::lab_defaults("delete-roundtrip");
        let path = recipe.save(&palettes_dir()).unwrap();
        assert!(path.is_file());
        assert!(list_recipe_names()
            .unwrap()
            .iter()
            .any(|name| name == "delete-roundtrip"));

        delete_named("delete-roundtrip").unwrap();
        assert!(!path.exists());
        assert!(!list_recipe_names()
            .unwrap()
            .iter()
            .any(|name| name == "delete-roundtrip"));
        assert!(delete_named("delete-roundtrip").is_err());
        assert!(delete_named("***").is_err());

        let _ = fs::remove_dir_all(root);
    }
}
