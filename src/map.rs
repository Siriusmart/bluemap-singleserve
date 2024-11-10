use std::collections::HashMap;
use std::error::Error;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use actix_files::NamedFile;
use actix_web::HttpRequest;
use actix_web::{http::StatusCode, HttpResponse, HttpResponseBuilder};
use default_from_serde::SerdeDefault;
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::broadcast::{channel, Sender};

use crate::{Config, MasterConfig};

// source - destination - template - dimension
#[allow(clippy::type_complexity)]
static mut LOCKS: OnceLock<
    HashMap<(PathBuf, PathBuf, PathBuf, Dimension), Sender<Result<(), MapError>>>,
> = OnceLock::new();

#[allow(non_snake_case)]
#[serde_inline_default]
#[derive(Serialize, Deserialize, SerdeDefault)]
pub struct SettingsGlobal {
    #[serde_inline_default("5.4".to_string())]
    pub version: String,
    #[serde_inline_default(true)]
    pub useCookies: bool,
    #[serde_inline_default(true)]
    pub enableFreeFlight: bool,
    #[serde_inline_default(false)]
    pub defaultToFlatView: bool,
    #[serde_inline_default(1)]
    pub resolutionDefault: u32,
    #[serde_inline_default(5)]
    pub minZoomDistance: u32,
    #[serde_inline_default(100000)]
    pub maxZoomDistance: u32,
    #[serde_inline_default(500)]
    pub hiresSliderMax: u32,
    #[serde_inline_default(100)]
    pub hiresSliderDefault: u32,
    #[serde_inline_default(0)]
    pub hiresSliderMin: u32,
    #[serde_inline_default(7000)]
    pub lowresSliderMax: u32,
    #[serde_inline_default(2000)]
    pub lowresSliderDefault: u32,
    #[serde_inline_default(500)]
    pub lowresSliderMin: u32,
    #[serde(default)]
    pub maps: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    #[serde(default)]
    pub styles: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum MapError {
    UnzipFailed,
    ConfigTemplateNotFound,
    RenderingFiled,
    DestinationExist,
    External { reason: String },
}

impl Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnzipFailed => "unzip failed",
            Self::ConfigTemplateNotFound => "config template not found",
            Self::RenderingFiled => "rendering failed",
            Self::DestinationExist => "destination exist",
            Self::External { reason } => reason,
        })
    }
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Display for Dimension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Overworld => "overworld",
            Self::Nether => "nether",
            Self::End => "end",
        })
    }
}

impl Error for MapError {}

pub struct Map;

impl Map {
    pub async fn exists(map_path: &Path) -> bool {
        fs::try_exists(map_path.join("settings.json"))
            .await
            .unwrap_or(false)
    }

    pub async fn render(
        source: &Path,
        dest: &Path,
        template: &Path,
        dimension: Dimension,
    ) -> Result<(), MapError> {
        let locks = Self::locks();
        let key = (
            source.to_path_buf(),
            dest.to_path_buf(),
            template.to_path_buf(),
            dimension,
        );

        if let Some(tx) = locks.get(&key) {
            return match tx.subscribe().recv().await {
                Ok(res) => res,
                Err(e) => Err(MapError::External {
                    reason: e.to_string(),
                }),
            };
        }

        let channel = channel(1).0;
        locks.insert(key.clone(), channel.clone());

        let res = match Self::render_internal(source, dest, template, dimension).await {
            Ok(res) => Ok(res),
            Err(e) => {
                if let Some(e) = e.downcast_ref::<MapError>() {
                    Err(e.clone())
                } else {
                    Err(MapError::External {
                        reason: e.to_string(),
                    })
                }
            }
        };

        let _ = channel.send(res.clone());
        let _ = locks.remove(&key);

        res
    }

    #[allow(clippy::type_complexity)]
    fn locks(
    ) -> &'static mut HashMap<(PathBuf, PathBuf, PathBuf, Dimension), Sender<Result<(), MapError>>>
    {
        if let Some(locks) = unsafe { LOCKS.get_mut() } {
            locks
        } else {
            let _ = unsafe { LOCKS.set(HashMap::new()) };
            Self::locks()
        }
    }

    async fn render_internal(
        source: &Path,
        dest: &Path,
        template: &Path,
        dimension: Dimension,
    ) -> Result<(), Box<dyn Error>> {
        if fs::try_exists(dest).await? {
            return Err(MapError::DestinationExist.into());
        }

        let master = MasterConfig::get();
        let id = fastrand::u64(..).to_string();

        let temp_zip = master.maps.join(&id).with_extension("zip");

        if !fs::try_exists(temp_zip.parent().unwrap()).await? {
            fs::create_dir_all(temp_zip.parent().unwrap()).await?;
        }

        let temp_zip_dir = temp_zip.with_extension("");

        fs::copy(source, &temp_zip).await?;
        let unzip = Command::new("unzip")
            .args([
                temp_zip.to_str().unwrap(),
                "-d",
                temp_zip_dir.to_str().unwrap(),
            ])
            .output()
            .await?;

        let _ = fs::remove_file(&temp_zip).await;

        if !unzip.status.success() {
            let _ = fs::remove_dir_all(temp_zip_dir).await;
            return Err(MapError::UnzipFailed.into());
        }

        let mut dir = fs::read_dir(&temp_zip_dir).await?;
        let mut items = Vec::new();

        while let Some(direntry) = dir.next_entry().await? {
            items.push(direntry.path());
            if items.len() == 2 {
                break;
            }
        }

        if let &[item] = &items.as_slice() {
            fs::rename(
                item,
                temp_zip_dir.with_file_name(format!(
                    "{}_temp",
                    item.file_name().unwrap().to_string_lossy()
                )),
            )
            .await?;
            fs::remove_dir(&temp_zip_dir).await?;
            fs::rename(
                temp_zip_dir.with_file_name(format!(
                    "{}_temp",
                    item.file_name().unwrap().to_string_lossy()
                )),
                temp_zip_dir,
            )
            .await?;
        }

        let config = match fs::read_to_string(template).await {
            Ok(file) => file,
            Err(_e) => return Err(MapError::ConfigTemplateNotFound.into()),
        }
        .replacen("%world%", temp_zip.with_extension("").to_str().unwrap(), 1)
        .replacen("%dimension%", dimension.to_string().as_str(), 1)
        .replacen("%name%", dest.file_name().unwrap().to_str().unwrap(), 1);

        let conf = master
            .bluemap_config
            .join("maps")
            .join(&id)
            .with_extension("conf");

        if !fs::try_exists(conf.parent().unwrap()).await? {
            fs::create_dir_all(conf.parent().unwrap()).await?;
        }

        let mut conf_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&conf)
            .await?;

        conf_file.write_all(config.as_bytes()).await?;

        let bluemap = Command::new("java")
            .args([
                "-jar",
                master.bluemap_jar.to_str().unwrap(),
                "-c",
                master.bluemap_config.to_str().unwrap(),
                "-m",
                id.as_str(),
                "-r",
            ])
            .output()
            .await?;

        let rendered = master.bluemap_web.join("maps").join(id);

        let _ = fs::remove_dir_all(temp_zip.with_extension("").to_str().unwrap()).await;
        let _ = fs::remove_file(conf).await;

        if !bluemap.status.success() {
            let _ = fs::remove_dir_all(rendered).await;
            return Err(MapError::RenderingFiled.into());
        }

        if !fs::try_exists(&dest.parent().unwrap()).await? {
            fs::create_dir_all(dest.parent().unwrap()).await?;
        }

        Ok(fs::rename(rendered, dest).await?)
    }

    pub async fn clean() {
        let master = MasterConfig::get();
        let _ = fs::remove_dir_all(&master.maps).await;
        let _ = fs::remove_dir_all(&master.bluemap_web.join("maps")).await;
    }

    pub async fn serve(
        map_path: &Path,
        req_path: &Path,
        req: &HttpRequest,
    ) -> Result<HttpResponse, Box<dyn Error>> {
        Ok(
            match req_path
                .iter()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>()
                .as_slice()
            {
                [] | ["index.html"] | ["lang", ..] | ["assets", ..] => {
                    let req_path = if req_path.iter().count() == 0 {
                        PathBuf::from("index.html")
                    } else {
                        req_path.to_path_buf()
                    };

                    NamedFile::open_async(&MasterConfig::get().bluemap_web.join(req_path))
                        .await?
                        .into_response(req)
                }
                ["maps", _, ..]
                    if fs::try_exists(
                        map_path.join(req_path.iter().skip(2).collect::<PathBuf>()),
                    )
                    .await? =>
                {
                    NamedFile::open_async(
                        map_path.join(req_path.iter().skip(2).collect::<PathBuf>()),
                    )
                    .await?
                    .into_response(req)
                }
                ["maps", _, ..]
                    if fs::try_exists(
                        map_path.join(req_path.iter().skip(2).collect::<PathBuf>().with_file_name(
                            format!("{}.gz", req_path.file_name().unwrap().to_string_lossy()),
                        )),
                    )
                    .await? =>
                {
                    NamedFile::open_async(
                        map_path.join(req_path.iter().skip(2).collect::<PathBuf>().with_file_name(
                            format!("{}.gz", req_path.file_name().unwrap().to_string_lossy()),
                        )),
                    )
                    .await?
                    .set_content_encoding(actix_web::http::header::ContentEncoding::Gzip)
                    .into_response(req)
                }
                ["settings.json"] => {
                    let settings = SettingsGlobal {
                        maps: vec![map_path.file_name().unwrap().to_string_lossy().to_string()],
                        ..Default::default()
                    };
                    HttpResponseBuilder::new(StatusCode::OK).json(settings)
                }
                _ => HttpResponseBuilder::new(StatusCode::NOT_FOUND).await?,
            },
        )
    }
}
