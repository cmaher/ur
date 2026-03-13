pub use ur_config::{
    DEFAULT_EMBEDDING_MODEL, ModelDownloadInfo, model_download_info, supported_model_names,
};

/// Full model info including the fastembed enum variant.
///
/// Combines the download metadata from `ur_config` with the fastembed-specific
/// model enum needed by the server at runtime.
pub struct ModelInfo {
    /// fastembed enum variant for initializing the model.
    pub fastembed_model: fastembed::EmbeddingModel,
    /// Download and dimension metadata.
    pub download: &'static ModelDownloadInfo,
}

/// Look up full model info (including fastembed variant) by config name.
///
/// Returns `None` for unknown model names.
pub fn model_info(name: &str) -> Option<ModelInfo> {
    let download = model_download_info(name)?;
    let fastembed_model = match name {
        "all-MiniLM-L6-v2" => fastembed::EmbeddingModel::AllMiniLML6V2,
        _ => return None,
    };
    Some(ModelInfo {
        fastembed_model,
        download,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_valid_model() {
        let info = model_info("all-MiniLM-L6-v2").unwrap();
        assert_eq!(info.download.vector_size, 384);
        assert_eq!(info.download.hf_org, "Qdrant");
        assert_eq!(info.download.hf_repo, "all-MiniLM-L6-v2-onnx");
        assert!(!info.download.hf_files.is_empty());
    }

    #[test]
    fn lookup_unknown_model_returns_none() {
        assert!(model_info("nonexistent-model").is_none());
    }

    #[test]
    fn default_model_is_valid() {
        assert!(model_info(DEFAULT_EMBEDDING_MODEL).is_some());
    }

    #[test]
    fn supported_names_includes_default() {
        let names = supported_model_names();
        assert!(names.contains(&DEFAULT_EMBEDDING_MODEL));
    }

    #[test]
    fn download_info_available_without_fastembed() {
        let info = model_download_info("all-MiniLM-L6-v2").unwrap();
        assert_eq!(info.vector_size, 384);
        assert_eq!(info.hf_commit, "5f1b8cd78bc4fb444dd171e59b18f3a3af89a079");
    }
}
