use crate::cache::docgen::DocGenerator;
use crate::cache::downloader::{CrateDownloader, CrateSource};
use crate::cache::storage::CacheStorage;
use crate::cache::transaction::CacheTransaction;
use crate::cache::utils::CacheResponse;
use crate::cache::workspace::WorkspaceHandler;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;

/// Service for managing crate caching and documentation generation
#[derive(Debug, Clone)]
pub struct CrateCache {
    pub(crate) storage: CacheStorage,
    downloader: CrateDownloader,
    doc_generator: DocGenerator,
}

impl CrateCache {
    /// Create a new crate cache instance
    pub fn new(cache_dir: Option<PathBuf>) -> Result<Self> {
        let storage = CacheStorage::new(cache_dir)?;
        let downloader = CrateDownloader::new(storage.clone());
        let doc_generator = DocGenerator::new(storage.clone());

        Ok(Self {
            storage,
            downloader,
            doc_generator,
        })
    }

    /// Ensure a crate's documentation is available, downloading and generating if necessary
    pub async fn ensure_crate_docs(
        &self,
        name: &str,
        version: &str,
        source: Option<&str>,
    ) -> Result<rustdoc_types::Crate> {
        // Check if docs already exist
        if self.storage.has_docs(name, version) {
            return self.load_docs(name, version).await;
        }

        // Check if crate is downloaded but docs not generated
        if !self.storage.is_cached(name, version) {
            self.download_or_copy_crate(name, version, source).await?;
        }

        // Generate documentation
        self.generate_docs(name, version).await?;

        // Load and return the generated docs
        self.load_docs(name, version).await
    }

    /// Ensure a workspace member's documentation is available
    pub async fn ensure_workspace_member_docs(
        &self,
        name: &str,
        version: &str,
        source: Option<&str>,
        member_path: &str,
    ) -> Result<rustdoc_types::Crate> {
        // Check if docs already exist for this member
        let member_name = WorkspaceHandler::extract_member_name(member_path);

        if self.storage.has_member_docs(name, version, member_name) {
            return self.load_member_docs(name, version, member_name).await;
        }

        // Check if crate is downloaded
        if !self.storage.is_cached(name, version) {
            self.download_or_copy_crate(name, version, source).await?;
        }

        // Generate documentation for the specific workspace member
        self.generate_workspace_member_docs(name, version, member_path)
            .await?;

        // Load and return the generated docs
        self.load_member_docs(name, version, member_name).await
    }

    /// Ensure documentation is available for a crate or workspace member
    pub async fn ensure_crate_or_member_docs(
        &self,
        name: &str,
        version: &str,
        member: Option<&str>,
    ) -> Result<rustdoc_types::Crate> {
        // If member is specified, use workspace member logic
        if let Some(member_path) = member {
            return self
                .ensure_workspace_member_docs(name, version, None, member_path)
                .await;
        }

        // Check if crate is already downloaded
        if self.storage.is_cached(name, version) {
            let source_path = self.storage.source_path(name, version);
            let cargo_toml_path = source_path.join("Cargo.toml");

            // Check if it's a workspace
            if cargo_toml_path.exists() && WorkspaceHandler::is_workspace(&cargo_toml_path)? {
                // It's a workspace without member specified
                let members = WorkspaceHandler::get_workspace_members(&cargo_toml_path)?;
                bail!(
                    "This is a workspace crate. Please specify a member using the 'member' parameter.\n\
                    Available members: {:?}\n\
                    Example: specify member=\"{}\"",
                    members,
                    members.first().unwrap_or(&"crates/example".to_string())
                );
            }
        }

        // Regular crate, use normal flow
        self.ensure_crate_docs(name, version, None).await
    }

    /// Download or copy a crate based on source type
    pub async fn download_or_copy_crate(
        &self,
        name: &str,
        version: &str,
        source: Option<&str>,
    ) -> Result<PathBuf> {
        self.downloader
            .download_or_copy_crate(name, version, source)
            .await
    }

    /// Generate JSON documentation for a crate
    pub async fn generate_docs(&self, name: &str, version: &str) -> Result<PathBuf> {
        self.doc_generator.generate_docs(name, version).await
    }

    /// Generate JSON documentation for a workspace member
    pub async fn generate_workspace_member_docs(
        &self,
        name: &str,
        version: &str,
        member_path: &str,
    ) -> Result<PathBuf> {
        self.doc_generator
            .generate_workspace_member_docs(name, version, member_path)
            .await
    }

    /// Load documentation from cache
    pub async fn load_docs(&self, name: &str, version: &str) -> Result<rustdoc_types::Crate> {
        let json_value = self.doc_generator.load_docs(name, version).await?;
        let crate_docs: rustdoc_types::Crate =
            serde_json::from_value(json_value).context("Failed to parse documentation JSON")?;
        Ok(crate_docs)
    }

    /// Load workspace member documentation from cache
    pub async fn load_member_docs(
        &self,
        name: &str,
        version: &str,
        member_name: &str,
    ) -> Result<rustdoc_types::Crate> {
        let json_value = self
            .doc_generator
            .load_member_docs(name, version, member_name)
            .await?;
        let crate_docs: rustdoc_types::Crate = serde_json::from_value(json_value)
            .context("Failed to parse member documentation JSON")?;
        Ok(crate_docs)
    }

    /// Get cached versions of a crate
    pub async fn get_cached_versions(&self, name: &str) -> Result<Vec<String>> {
        let cached = self.storage.list_cached_crates()?;
        let versions: Vec<String> = cached
            .into_iter()
            .filter(|meta| meta.name == name)
            .map(|meta| meta.version)
            .collect();

        Ok(versions)
    }

    /// Get all cached crates with their metadata
    pub async fn list_all_cached_crates(
        &self,
    ) -> Result<Vec<crate::cache::storage::CrateMetadata>> {
        self.storage.list_cached_crates()
    }

    /// Remove a cached crate version
    pub async fn remove_crate(&self, name: &str, version: &str) -> Result<()> {
        self.storage.remove_crate(name, version)
    }

    /// Get the source path for a crate
    pub fn get_source_path(&self, name: &str, version: &str) -> PathBuf {
        self.storage.source_path(name, version)
    }

    /// Ensure a crate's source is available, downloading if necessary (without generating docs)
    pub async fn ensure_crate_source(
        &self,
        name: &str,
        version: &str,
        source: Option<&str>,
    ) -> Result<PathBuf> {
        // Check if crate is already downloaded
        if !self.storage.is_cached(name, version) {
            self.download_or_copy_crate(name, version, source).await?;
        }

        Ok(self.storage.source_path(name, version))
    }

    /// Ensure source is available for a crate or workspace member
    pub async fn ensure_crate_or_member_source(
        &self,
        name: &str,
        version: &str,
        member: Option<&str>,
        source: Option<&str>,
    ) -> Result<PathBuf> {
        // Ensure the crate source is downloaded
        let source_path = self.ensure_crate_source(name, version, source).await?;

        // If member is specified, return the member's source path
        if let Some(member_path) = member {
            let member_source_path = source_path.join(member_path);
            let member_cargo_toml = member_source_path.join("Cargo.toml");

            if !member_cargo_toml.exists() {
                bail!(
                    "Workspace member '{}' not found in {}-{}. \
                    Make sure the member path is correct.",
                    member_path,
                    name,
                    version
                );
            }

            return Ok(member_source_path);
        }

        // Check if it's a workspace without member specified
        let cargo_toml_path = source_path.join("Cargo.toml");
        if cargo_toml_path.exists() && WorkspaceHandler::is_workspace(&cargo_toml_path)? {
            let members = WorkspaceHandler::get_workspace_members(&cargo_toml_path)?;
            bail!(
                "This is a workspace crate. Please specify a member using the 'member' parameter.\n\
                Available members: {:?}\n\
                Example: specify member=\"{}\"",
                members,
                members.first().unwrap_or(&"crates/example".to_string())
            );
        }

        // Regular crate, return source path
        Ok(source_path)
    }

    /// Load dependency information from cache
    pub async fn load_dependencies(&self, name: &str, version: &str) -> Result<serde_json::Value> {
        self.doc_generator.load_dependencies(name, version).await
    }

    /// Internal implementation for caching a crate during update
    async fn cache_crate_with_update_impl(
        &self,
        crate_name: &str,
        version: &str,
        members: &Option<Vec<String>>,
        source_str: Option<&str>,
        source: &CrateSource,
    ) -> Result<CacheResponse> {
        // If members are specified, cache those specific workspace members
        if let Some(members) = members {
            let response = self
                .cache_workspace_members(crate_name, version, members, source_str, true)
                .await;

            // Check if all failed for proper error handling
            if let CacheResponse::PartialSuccess {
                results, errors, ..
            } = &response
                && results.is_empty()
            {
                bail!("Failed to update any workspace members: {:?}", errors);
            }

            return Ok(response);
        }

        // Download the crate
        let source_path = self
            .download_or_copy_crate(crate_name, version, source_str)
            .await?;

        // Check if it's a workspace
        let cargo_toml_path = source_path.join("Cargo.toml");
        if WorkspaceHandler::is_workspace(&cargo_toml_path)? {
            // It's a workspace, get the members
            let members = WorkspaceHandler::get_workspace_members(&cargo_toml_path)?;
            Ok(self.generate_workspace_response(crate_name, version, members, source, true))
        } else {
            // Not a workspace, proceed with normal caching
            self.ensure_crate_docs(crate_name, version, source_str)
                .await?;

            Ok(CacheResponse::success_updated(crate_name, version))
        }
    }

    /// Extract source parameters from CrateSource enum
    fn extract_source_params(
        &self,
        source: &CrateSource,
    ) -> (String, String, Option<Vec<String>>, Option<String>, bool) {
        match source {
            CrateSource::CratesIO(params) => (
                params.crate_name.clone(),
                params.version.clone(),
                params.members.clone(),
                None,
                params.update.unwrap_or(false),
            ),
            CrateSource::GitHub(params) => {
                let version = if let Some(branch) = &params.branch {
                    branch.clone()
                } else if let Some(tag) = &params.tag {
                    tag.clone()
                } else {
                    // This should not happen due to validation in the tool layer
                    String::new()
                };

                let source_str = if let Some(branch) = &params.branch {
                    Some(format!("{}#branch:{branch}", params.github_url))
                } else if let Some(tag) = &params.tag {
                    Some(format!("{}#tag:{tag}", params.github_url))
                } else {
                    Some(params.github_url.clone())
                };

                (
                    params.crate_name.clone(),
                    version,
                    params.members.clone(),
                    source_str,
                    params.update.unwrap_or(false),
                )
            }
            CrateSource::LocalPath(params) => (
                params.crate_name.clone(),
                params.version.clone(),
                params.members.clone(),
                Some(params.path.clone()),
                params.update.unwrap_or(false),
            ),
        }
    }

    /// Handle caching workspace members
    async fn cache_workspace_members(
        &self,
        crate_name: &str,
        version: &str,
        members: &[String],
        source_str: Option<&str>,
        updated: bool,
    ) -> CacheResponse {
        use futures::future::join_all;

        // Create futures for all member caching operations
        let member_futures: Vec<_> = members
            .iter()
            .map(|member| {
                let member_clone = member.clone();
                async move {
                    let result = self
                        .ensure_workspace_member_docs(
                            crate_name,
                            version,
                            source_str,
                            &member_clone,
                        )
                        .await;
                    (member_clone, result)
                }
            })
            .collect();

        // Execute all futures concurrently
        let results_with_members = join_all(member_futures).await;

        // Collect results and errors
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for (member, result) in results_with_members {
            match result {
                Ok(_) => {
                    results.push(format!("Successfully cached member: {member}"));
                }
                Err(e) => {
                    errors.push(format!("Failed to cache member {member}: {e}"));
                }
            }
        }

        if errors.is_empty() {
            CacheResponse::members_success(crate_name, version, members.to_vec(), results, updated)
        } else {
            CacheResponse::members_partial(
                crate_name,
                version,
                members.to_vec(),
                results,
                errors,
                updated,
            )
        }
    }

    /// Generate workspace detection response
    fn generate_workspace_response(
        &self,
        crate_name: &str,
        version: &str,
        members: Vec<String>,
        source: &CrateSource,
        updated: bool,
    ) -> CacheResponse {
        let source_type = match source {
            CrateSource::CratesIO(_) => "cratesio",
            CrateSource::GitHub(_) => "github",
            CrateSource::LocalPath(_) => "local",
        };

        CacheResponse::workspace_detected(crate_name, version, members, source_type, updated)
    }

    /// Handle update operation for a crate
    async fn handle_crate_update(
        &self,
        crate_name: &str,
        version: &str,
        members: &Option<Vec<String>>,
        source_str: Option<&str>,
        source: &CrateSource,
    ) -> String {
        // Create transaction for safe update
        let mut transaction = CacheTransaction::new(&self.storage, crate_name, version);

        // Begin transaction (creates backup and removes existing cache)
        if let Err(e) = transaction.begin() {
            return CacheResponse::error(format!("Failed to start update transaction: {e}"))
                .to_json();
        }

        // Try to re-cache the crate
        let update_result = self
            .cache_crate_with_update_impl(crate_name, version, members, source_str, source)
            .await;

        // Check if update was successful
        match update_result {
            Ok(response) => {
                // Success - commit transaction
                if let Err(e) = transaction.commit() {
                    return CacheResponse::error(format!(
                        "Update succeeded but failed to cleanup: {e}"
                    ))
                    .to_json();
                }
                response.to_json()
            }
            Err(e) => {
                // Failed - transaction will automatically rollback on drop
                CacheResponse::error(format!("Update failed, restored from backup: {e}")).to_json()
            }
        }
    }

    /// Handle workspace members caching
    async fn handle_workspace_members(
        &self,
        crate_name: &str,
        version: &str,
        members: &[String],
        source_str: Option<&str>,
        updated: bool,
    ) -> CacheResponse {
        self.cache_workspace_members(crate_name, version, members, source_str, updated)
            .await
    }

    /// Detect and handle workspace crates
    async fn detect_and_handle_workspace(
        &self,
        crate_name: &str,
        version: &str,
        source_path: &std::path::Path,
        source: &CrateSource,
        source_str: Option<&str>,
        updated: bool,
    ) -> Result<CacheResponse> {
        let cargo_toml_path = source_path.join("Cargo.toml");

        match WorkspaceHandler::is_workspace(&cargo_toml_path) {
            Ok(true) => {
                // It's a workspace, get the members
                let members = WorkspaceHandler::get_workspace_members(&cargo_toml_path)
                    .context("Failed to get workspace members")?;
                Ok(self.generate_workspace_response(crate_name, version, members, source, updated))
            }
            Ok(false) => {
                // Not a workspace, proceed with normal caching
                self.cache_regular_crate(crate_name, version, source_str)
                    .await
            }
            Err(_e) => {
                // Error checking workspace status, try normal caching anyway
                self.cache_regular_crate(crate_name, version, source_str)
                    .await
            }
        }
    }

    /// Cache a regular (non-workspace) crate
    async fn cache_regular_crate(
        &self,
        crate_name: &str,
        version: &str,
        source_str: Option<&str>,
    ) -> Result<CacheResponse> {
        self.ensure_crate_docs(crate_name, version, source_str)
            .await
            .context("Failed to cache crate")?;
        Ok(CacheResponse::success(crate_name, version))
    }

    /// Common method to cache a crate from any source
    pub async fn cache_crate_with_source(&self, source: CrateSource) -> String {
        // Extract parameters from source
        let (crate_name, version, members, source_str, update) =
            self.extract_source_params(&source);

        // Validate GitHub source
        if matches!(&source, CrateSource::GitHub(_)) && version.is_empty() {
            return CacheResponse::error("Either branch or tag must be specified").to_json();
        }

        // Handle update logic if requested
        if update && self.storage.is_cached(&crate_name, &version) {
            return self
                .handle_crate_update(
                    &crate_name,
                    &version,
                    &members,
                    source_str.as_deref(),
                    &source,
                )
                .await;
        }

        // If members are specified, cache those specific workspace members
        if let Some(members) = members {
            let response = self
                .handle_workspace_members(
                    &crate_name,
                    &version,
                    &members,
                    source_str.as_deref(),
                    false,
                )
                .await;
            return response.to_json();
        }

        // First, download the crate if not already cached
        let source_path = match self
            .download_or_copy_crate(&crate_name, &version, source_str.as_deref())
            .await
        {
            Ok(path) => path,
            Err(e) => {
                return CacheResponse::error(format!("Failed to download crate: {e}")).to_json();
            }
        };

        // Detect and handle workspace vs regular crate
        match self
            .detect_and_handle_workspace(
                &crate_name,
                &version,
                &source_path,
                &source,
                source_str.as_deref(),
                false,
            )
            .await
        {
            Ok(response) => response.to_json(),
            Err(e) => CacheResponse::error(format!("Failed to cache crate: {e}")).to_json(),
        }
    }
}
