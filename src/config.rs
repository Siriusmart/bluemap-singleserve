use std::{fs, io::Write, path::PathBuf, sync::OnceLock};

use default_from_serde::SerdeDefault;
use dirs::config_dir;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_inline_default::serde_inline_default;

static MASTER_CONFIG: OnceLock<MasterConfig> = OnceLock::new();

#[serde_inline_default]
#[derive(Serialize, Deserialize, SerdeDefault)]
pub struct MasterConfig {
    #[serde_inline_default(PathBuf::from("config"))]
    pub bluemap_config: PathBuf,
    #[serde_inline_default(PathBuf::from("web"))]
    pub bluemap_web: PathBuf,
    #[serde_inline_default(PathBuf::from("bluemap.jar"))]
    pub bluemap_jar: PathBuf,
    #[serde_inline_default(PathBuf::from("artifacts"))]
    pub artifacts: PathBuf,
}

impl Config for MasterConfig {
    fn ident() -> &'static str {
        "master"
    }

    fn oncelock() -> &'static OnceLock<Self> {
        &MASTER_CONFIG
    }
}

pub trait Config: Serialize + DeserializeOwned + Default
where
    Self: 'static,
{
    fn ident() -> &'static str;
    fn oncelock() -> &'static OnceLock<Self>;

    fn path() -> PathBuf {
        config_dir()
            .unwrap()
            .join("bluemap")
            .join(Self::ident())
            .with_extension("json")
    }

    fn save(&self) {
        let content = serde_json::to_vec(&self).unwrap();
        let path = Self::path();

        if !path.parent().unwrap().exists() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
        }

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();

        file.write_all(&content).unwrap();
    }

    fn load() {
        let path = Self::path();

        if !path.exists() {
            let def = Self::default();
            def.save();
            let _ = Self::oncelock().set(def);
            return;
        }

        let content = fs::read_to_string(path).unwrap();

        let _ = Self::oncelock().set(if let Ok(val) = serde_json::from_str::<Self>(&content) {
            val
        } else {
            let def = Self::default();
            def.save();
            def
        });
    }

    fn get() -> &'static Self {
        Self::oncelock().get().unwrap()
    }
}
