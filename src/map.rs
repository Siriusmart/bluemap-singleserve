use std::error::Error;
use std::path::{Path, PathBuf};

use actix_files::NamedFile;
use actix_web::HttpRequest;
use actix_web::{http::StatusCode, HttpResponse, HttpResponseBuilder};
use default_from_serde::SerdeDefault;
use mime::Mime;
use serde::{Deserialize, Serialize};
use serde_inline_default::serde_inline_default;
use tokio::fs;

use crate::{Config, MasterConfig};

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

pub struct Map;

impl Map {
    pub async fn exists(map_path: &Path) -> bool {
        fs::try_exists(
            &MasterConfig::get()
                .artifacts
                .join(map_path)
                .join("settings.json"),
        )
        .await
        .unwrap()
    }

    pub async fn serve(
        map_path: &Path,
        req_path: &Path,
        req: &HttpRequest,
    ) -> Result<HttpResponse, Box<dyn Error>> {
        let abs_map_path = MasterConfig::get().artifacts.join(map_path);

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
                        abs_map_path.join(req_path.iter().skip(2).collect::<PathBuf>()),
                    )
                    .await? =>
                {
                    NamedFile::open_async(
                        abs_map_path.join(req_path.iter().skip(2).collect::<PathBuf>()),
                    )
                    .await?
                    .into_response(req)
                }
                ["maps", _, ..]
                    if fs::try_exists(
                        abs_map_path.join(
                            req_path
                                .iter()
                                .skip(2)
                                .collect::<PathBuf>()
                                .with_file_name(format!(
                                    "{}.gz",
                                    req_path.file_name().unwrap().to_string_lossy()
                                )),
                        ),
                    )
                    .await? =>
                {
                    NamedFile::open_async(
                        abs_map_path.join(
                            req_path
                                .iter()
                                .skip(2)
                                .collect::<PathBuf>()
                                .with_file_name(format!(
                                    "{}.gz",
                                    req_path.file_name().unwrap().to_string_lossy()
                                )),
                        ),
                    )
                    .await?
                    .set_content_encoding(actix_web::http::header::ContentEncoding::Gzip)
                    .into_response(req)
                }
                ["settings.json"] => {
                    let mut settings = SettingsGlobal::default();
                    settings.maps =
                        vec![map_path.file_name().unwrap().to_string_lossy().to_string()];
                    HttpResponseBuilder::new(StatusCode::OK).json(settings)
                }
                _ => HttpResponseBuilder::new(StatusCode::NOT_FOUND).await?,
            },
        )
    }
}
