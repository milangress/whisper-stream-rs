use std::path::{PathBuf, Path};
use std::fs;
use std::io::{self, Write};
use crate::error::WhisperStreamError;
use log::{info};
use std::fmt;
use std::str::FromStr;

#[cfg(feature = "coreml")]
use zip::ZipArchive;
#[cfg(feature = "coreml")]
use std::fs::File;
#[cfg(feature = "coreml")]
use log::{warn};

/// Supported Whisper models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// The default model: base.en
    BaseEn,
    /// The tiny.en model
    TinyEn,
    /// The small.en model
    SmallEn,
}

impl Model {
    /// Returns the user-facing name for this model (e.g., "base.en").
    pub fn name(&self) -> &'static str {
        match self {
            Model::BaseEn => "base.en",
            Model::TinyEn => "tiny.en",
            Model::SmallEn => "small.en",
        }
    }
    /// Returns the model file name (e.g., "ggml-base.en.bin").
    pub fn file_name(&self) -> &'static str {
        match self {
            Model::BaseEn => "ggml-base.en.bin",
            Model::TinyEn => "ggml-tiny.en.bin",
            Model::SmallEn => "ggml-small.en.bin",
        }
    }
    /// Returns the model download URL.
    pub fn url(&self) -> &'static str {
        match self {
            Model::BaseEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
            Model::TinyEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
            Model::SmallEn => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
        }
    }
    /// Returns all supported models.
    pub fn list() -> Vec<Model> {
        vec![Model::BaseEn, Model::TinyEn, Model::SmallEn]
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for Model {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "base.en" => Ok(Model::BaseEn),
            "tiny.en" => Ok(Model::TinyEn),
            "small.en" => Ok(Model::SmallEn),
            _ => Err(()),
        }
    }
}


#[cfg(feature = "coreml")]
const COREML_MODEL_URL_TEMPLATE: &str = "https://models.milan.place/whisper-cpp/metal//{}-encoder.mlmodelc.zip";
#[cfg(feature = "coreml")]
const BASE_MODEL_NAME_FOR_COREML: &str = "ggml-base.en"; // Corresponds to ggml-base.en.bin

/// Ensures the Whisper model (and CoreML model if 'coreml' feature is enabled) is present, downloading if necessary.
pub fn ensure_model(model: Model) -> Result<PathBuf, WhisperStreamError> {
    let cache_dir = dirs::data_local_dir()
        .ok_or_else(|| WhisperStreamError::Io {
            source: io::Error::new(io::ErrorKind::NotFound, "Could not find local data dir")
        })?
        .join("whisper-stream-rs");

    fs::create_dir_all(&cache_dir).map_err(WhisperStreamError::from)?;

    let model_path = cache_dir.join(model.file_name());

    if !model_path.exists() {
        info!("Downloading Whisper model to {}...", model_path.display());
        download_file(model.url(), &model_path)?;
        info!("Whisper model downloaded.");
    }

    #[cfg(feature = "coreml")]
    {
        ensure_coreml_model_if_enabled(&cache_dir)?;
    }

    Ok(model_path) // Return path to the main .bin model
}

#[cfg(feature = "coreml")]
fn ensure_coreml_model_if_enabled(cache_dir: &Path) -> Result<(), WhisperStreamError> {
    info!("CoreML feature enabled. Checking for CoreML model...");
    let coreml_base_name = BASE_MODEL_NAME_FOR_COREML;
    let coreml_encoder_dir_name = format!("{}-encoder.mlmodelc", coreml_base_name);
    let coreml_model_dir_path = cache_dir.join(&coreml_encoder_dir_name);

    if !coreml_model_dir_path.exists() {
        let coreml_model_zip_url = COREML_MODEL_URL_TEMPLATE.replace("{}", coreml_base_name);
        let coreml_zip_filename = format!("{}-encoder.mlmodelc.zip", coreml_base_name);
        let coreml_zip_path = cache_dir.join(&coreml_zip_filename);

        info!("Downloading CoreML model from {} to {}...", coreml_model_zip_url, coreml_zip_path.display());
        download_file(&coreml_model_zip_url, &coreml_zip_path)?;
        info!("CoreML model ZIP downloaded.");

        info!("Unzipping CoreML model to {}...", cache_dir.display());
        if let Err(e) = unzip_file(&coreml_zip_path, &cache_dir) {
            // Attempt to clean up the potentially corrupted zip file or partial extraction
            if let Err(remove_err) = fs::remove_file(&coreml_zip_path) {
                warn!("Failed to remove zip file {} during cleanup: {}", coreml_zip_path.display(), remove_err);
            }
            if let Err(remove_dir_err) = fs::remove_dir_all(&coreml_model_dir_path) {
                warn!("Failed to remove directory {} during cleanup: {}", coreml_model_dir_path.display(), remove_dir_err);
            }
            // The error is returned from this function, so no need for error! here, caller handles it.
            return Err(e);
        }
        info!("CoreML model unzipped and available at {}.", coreml_model_dir_path.display());

        // Clean up the downloaded zip file after successful extraction
        if fs::remove_file(&coreml_zip_path).is_err() {
            warn!("Could not remove CoreML zip file: {}", coreml_zip_path.display());
        }
    } else {
        info!("CoreML model already present at {}.", coreml_model_dir_path.display());
    }
    Ok(())
}

fn download_file(url: &str, path: &Path) -> Result<(), WhisperStreamError> {
    let mut resp = reqwest::blocking::get(url)
        .map_err(|e| WhisperStreamError::ModelFetch(format!("Failed to initiate download from {}: {}", url, e)))?;

    if !resp.status().is_success() {
        return Err(WhisperStreamError::ModelFetch(format!("Failed to download from {}: HTTP Status {}", url, resp.status())));
    }

    let mut out = fs::File::create(path)
        .map_err(|e| WhisperStreamError::Io { source: e })?;

    io::copy(&mut resp, &mut out)
        .map_err(|e| WhisperStreamError::Io { source: e })?;

    out.flush().map_err(|e| WhisperStreamError::Io { source: e })?;
    Ok(())
}

#[cfg(feature = "coreml")]
fn unzip_file(zip_path: &Path, dest_dir: &Path) -> Result<(), WhisperStreamError> {
    let file = File::open(zip_path).map_err(|e| WhisperStreamError::Io { source: e })?;
    let mut archive = ZipArchive::new(file).map_err(|e| WhisperStreamError::ModelFetch(format!("Failed to open zip archive '{}': {}", zip_path.display(), e)))?;

    for i in 0..archive.len() {
        let mut file_in_zip = archive.by_index(i).map_err(|e| WhisperStreamError::ModelFetch(format!("Failed to access file in zip '{}': {}", zip_path.display(), e)))?;
        let outpath = match file_in_zip.enclosed_name() {
            Some(path) => dest_dir.join(path),
            None => continue, // Skip if path is risky (e.g. ../)
        };

        if file_in_zip.name().ends_with('/') {
            fs::create_dir_all(&outpath).map_err(|e| WhisperStreamError::Io { source: e })?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p).map_err(|e| WhisperStreamError::Io { source: e })?;
                }
            }
            let mut outfile = fs::File::create(&outpath).map_err(|e| WhisperStreamError::Io { source: e })?;
            io::copy(&mut file_in_zip, &mut outfile).map_err(|e| WhisperStreamError::Io { source: e })?;
        }
    }
    Ok(())
}