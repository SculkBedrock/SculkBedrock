use crate::manifest::SCPluginManifest;
use crate::{SCPlugin, SCPluginBoxer};
use std::collections::hash_map::DefaultHasher;
use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::io::{Error, Read, Seek};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use sc_binary::ByteReader;
use sc_error::{ScError, ScSite, capture_site};
use zip::read::ZipFile;
use zip::result::{ZipError, ZipResult};
use zip::ZipArchive;

static NEXT_PLUGIN_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

struct TemporaryLibraryPath(Option<PathBuf>);

impl Drop for TemporaryLibraryPath {
    fn drop(&mut self) {
        let Some(path) = self.0.take() else {
            return;
        };
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::debug!(
                    "plugin-loader >> failed to remove temporary library {}: {error}",
                    path.display()
                );
            }
        }
    }
}

pub(crate) fn get_file_by_name<'a, T: Read + Seek>(
    zip: &'a mut ZipArchive<T>,
    path: &str,
) -> ZipResult<ZipFile<'a, T>> {
    // Keep the file name in a temporary variable.
    let file_name = {
        zip.file_names()
            .find(|&name| name == path)
            .ok_or(ZipError::Io(Error::other("cannot get zip file")))?
            .to_string() // Convert to String.
    };
    zip.by_name(&file_name)
}

pub struct LoadedHostPlugin {
    pub plugin: sc_plugin_api::HostPluginV1,
    _library: Option<Arc<libloading::Library>>,
    _library_path: Option<TemporaryLibraryPath>,
}

impl LoadedHostPlugin {
    fn new(
        plugin: sc_plugin_api::HostPluginV1,
        library: Arc<libloading::Library>,
        library_path: PathBuf,
    ) -> Self {
        Self {
            plugin,
            _library: Some(library),
            _library_path: Some(TemporaryLibraryPath(Some(library_path))),
        }
    }
}

impl Drop for LoadedHostPlugin {
    fn drop(&mut self) {
        // The library remains alive while this body runs, so callbacks and
        // the plugin-owned destructor are still valid.
        unsafe {
            (self.plugin.disable)(self.plugin.context);
            (self.plugin.destroy)(self.plugin.context);
        }
    }
}

#[derive(Debug, ScError)]
pub enum SCPluginLoaderError {
    /// Zip archive failure.
    #[sc_error(code = "plugin.loader.zip")]
    ZipError {
        /// Underlying zip error.
        source: ZipError,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// File I/O failure.
    #[sc_error(code = "plugin.loader.io")]
    IoError {
        /// Underlying I/O error.
        source: Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Dynamic library failure.
    #[sc_error(code = "plugin.loader.lib")]
    LibError {
        /// Underlying library error.
        source: libloading::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Manifest JSON failure.
    #[sc_error(code = "plugin.loader.serde")]
    SerdeError {
        /// Underlying JSON error.
        source: serde_json::Error,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
    /// Host plugin ABI mismatch.
    #[sc_error(code = "plugin.loader.invalid_abi")]
    InvalidHostPluginAbi {
        /// Reported ABI version.
        abi: u32,
        /// English detail explaining the cause.
        detail: String,
        /// Construction site.
        site: ScSite,
    },
}

impl SCPluginLoaderError {
    /// Build a zip error, capturing the call site.
    #[track_caller]
    pub fn zip_error(source: ZipError) -> Self {
        let detail = source.to_string();
        Self::ZipError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build an I/O error, capturing the call site.
    #[track_caller]
    pub fn io_error(source: Error) -> Self {
        let detail = source.to_string();
        Self::IoError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build a library error, capturing the call site.
    #[track_caller]
    pub fn lib_error(source: libloading::Error) -> Self {
        let detail = source.to_string();
        Self::LibError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build a manifest JSON error, capturing the call site.
    #[track_caller]
    pub fn serde_error(source: serde_json::Error) -> Self {
        let detail = source.to_string();
        Self::SerdeError {
            source,
            detail,
            site: capture_site(),
        }
    }

    /// Build an ABI mismatch error, capturing the call site.
    #[track_caller]
    pub fn invalid_host_plugin_abi(abi: u32) -> Self {
        Self::InvalidHostPluginAbi {
            abi,
            detail: format!("unsupported host plugin ABI version {abi}"),
            site: capture_site(),
        }
    }
}

impl Display for SCPluginLoaderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZipError { detail, site, .. } => {
                write!(f, "[plugin.loader.zip] {detail} (at {site})")
            }
            Self::IoError { detail, site, .. } => {
                write!(f, "[plugin.loader.io] {detail} (at {site})")
            }
            Self::LibError { detail, site, .. } => {
                write!(f, "[plugin.loader.lib] {detail} (at {site})")
            }
            Self::SerdeError { detail, site, .. } => {
                write!(f, "[plugin.loader.serde] {detail} (at {site})")
            }
            Self::InvalidHostPluginAbi { detail, site, .. } => {
                write!(f, "[plugin.loader.invalid_abi] {detail} (at {site})")
            }
        }
    }
}

impl std::error::Error for SCPluginLoaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ZipError { source, .. } => Some(source),
            Self::IoError { source, .. } => Some(source),
            Self::LibError { source, .. } => Some(source),
            Self::SerdeError { source, .. } => Some(source),
            Self::InvalidHostPluginAbi { .. } => None,
        }
    }
}


#[allow(improper_ctypes_definitions)]
type CreateRustPluginFn = unsafe extern "C" fn() -> Box<dyn SCPlugin>;

pub struct SCPluginLoader;

impl SCPluginLoader {
    /// Reads the plugin manifest (without loading; bootstrap uses it to tell rust/cabi kinds apart).
    pub fn read_manifest<B: AsRef<[u8]>>(
        bytes: B,
    ) -> Result<SCPluginManifest, SCPluginLoaderError> {
        let mut zip = ZipArchive::new(ByteReader::from(bytes.as_ref()))
            .map_err(|e| SCPluginLoaderError::zip_error(e))?;
        let mut file = get_file_by_name(&mut zip, "manifest.json")
            .map_err(|e| SCPluginLoaderError::zip_error(e))?;
        let mut buffer = String::new();
        file.read_to_string(&mut buffer)
            .map_err(|e| SCPluginLoaderError::io_error(e))?;
        serde_json::from_str::<SCPluginManifest>(&buffer)
            .map_err(|e| SCPluginLoaderError::serde_error(e))
    }

    /// Loads a **host ABI** plugin (the path for version-pack plugins): zip -> plugin.so ->
    /// dlopen -> `get_host_plugin` (returns the fixed `HostPluginV1` table)
    /// -> `enable(&api)`. Plugins only depend on `sc_plugin_api` (C-ABI) and never link the sc_* core,
    /// avoiding duplicate instantiation segfaults with the host.
    pub fn load_rust_plugin<B: AsRef<[u8]>>(
        bytes: B,
        app: sc_ecs::app::App,
        manifest: SCPluginManifest,
    ) -> Result<SCPluginBoxer, SCPluginLoaderError> {
        let mut zip = ZipArchive::new(ByteReader::from(bytes.as_ref()))
            .map_err(|e| SCPluginLoaderError::zip_error(e))?;
        #[cfg(target_os = "windows")]
        let path = "plugin.dll";
        #[cfg(target_os = "linux")]
        let path = "plugin.so";
        #[cfg(target_os = "macos")]
        let path = "plugin.dylib";
        let mut file =
            get_file_by_name(&mut zip, path).map_err(|e| SCPluginLoaderError::zip_error(e))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| SCPluginLoaderError::io_error(e))?;

        let dir_path = std::env::temp_dir().join("sc_plugins");
        std::fs::create_dir_all(&dir_path).map_err(|e| SCPluginLoaderError::io_error(e))?;
        let mut hasher = DefaultHasher::new();
        buffer.hash(&mut hasher);
        let unique = hasher.finish();
        let instance = NEXT_PLUGIN_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        #[cfg(target_os = "windows")]
        let file_name = format!(
            "rust-plugin-{}-{unique:016x}-{instance:016x}.dll",
            std::process::id()
        );
        #[cfg(target_os = "linux")]
        let file_name = format!(
            "rust-plugin-{}-{unique:016x}-{instance:016x}.so",
            std::process::id()
        );
        #[cfg(target_os = "macos")]
        let file_name = format!(
            "rust-plugin-{}-{unique:016x}-{instance:016x}.dylib",
            std::process::id()
        );
        let file_path = dir_path.join(file_name);
        std::fs::write(&file_path, &buffer).map_err(|e| SCPluginLoaderError::io_error(e))?;

        let lib = match unsafe { libloading::Library::new(&file_path) } {
            Ok(lib) => Arc::new(lib),
            Err(error) => {
                let _ = std::fs::remove_file(&file_path);
                return Err(SCPluginLoaderError::lib_error(error));
            }
        };
        // Inject the log bridge (optional symbol exported via declare_log_bridge!).
        // Must run before get_plugin so log calls inside enable() already forward to the host.
        // Also passes the plugin name (manifest.name): the plugin side uses it as the log target,
        // so host output reads `... sc_vanilla_overworld >> message`.
        unsafe {
            if let Ok(install) = lib.get::<extern "C" fn(
                crate::log_bridge::LogBridgeFn,
                *const u8,
                usize,
            )>(b"sc_plugin_log_bridge\0")
            {
                let name = manifest.name.as_bytes();
                install(crate::log_bridge::host_log_bridge, name.as_ptr(), name.len());
                log::debug!("plugin-loader >> log bridge injected for {}", manifest.name);
            }
        }
        let get_plugin: libloading::Symbol<CreateRustPluginFn> =
            match unsafe { lib.get(b"get_plugin\0") } {
                Ok(symbol) => symbol,
                Err(error) => {
                    drop(lib);
                    let _ = std::fs::remove_file(&file_path);
                    return Err(SCPluginLoaderError::lib_error(error));
                }
            };
        let plugin = unsafe { get_plugin() };
        Ok(SCPluginBoxer::new_dynamic(
            plugin, app, manifest, lib, file_path,
        ))
    }

    pub fn load_host_plugin<B: AsRef<[u8]>>(
        bytes: B,
        api: &sc_plugin_api::HostApiV1,
    ) -> Result<LoadedHostPlugin, SCPluginLoaderError> {
        let mut zip = ZipArchive::new(ByteReader::from(bytes.as_ref()))
            .map_err(|e| SCPluginLoaderError::zip_error(e))?;
        #[cfg(target_os = "windows")]
        let path = "plugin.dll";
        #[cfg(target_os = "linux")]
        let path = "plugin.so";
        #[cfg(target_os = "macos")]
        let path = "plugin.dylib";
        let mut file =
            get_file_by_name(&mut zip, path).map_err(|e| SCPluginLoaderError::zip_error(e))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| SCPluginLoaderError::io_error(e))?;

        // The host side provides the temp dir (SCTempDir); here the file lands in the system temp dir.
        let dir_path = std::env::temp_dir().join("sc_plugins");
        std::fs::create_dir_all(&dir_path).map_err(|e| SCPluginLoaderError::io_error(e))?;
        let mut hasher = DefaultHasher::new();
        buffer.hash(&mut hasher);
        let unique = hasher.finish();
        let instance = NEXT_PLUGIN_INSTANCE_ID.fetch_add(1, Ordering::Relaxed);
        #[cfg(target_os = "windows")]
        let file_name = format!(
            "plugin-{}-{unique:016x}-{instance:016x}.dll",
            std::process::id()
        );
        #[cfg(target_os = "linux")]
        let file_name = format!(
            "plugin-{}-{unique:016x}-{instance:016x}.so",
            std::process::id()
        );
        #[cfg(target_os = "macos")]
        let file_name = format!(
            "plugin-{}-{unique:016x}-{instance:016x}.dylib",
            std::process::id()
        );
        let file_path = dir_path.join(file_name);
        std::fs::write(&file_path, &buffer).map_err(|e| SCPluginLoaderError::io_error(e))?;

        let lib = match unsafe { libloading::Library::new(&file_path) } {
            Ok(lib) => Arc::new(lib),
            Err(error) => {
                let _ = std::fs::remove_file(&file_path);
                return Err(SCPluginLoaderError::lib_error(error));
            }
        };
        let get_host_plugin: libloading::Symbol<sc_plugin_api::CreateHostPluginFn> =
            match unsafe { lib.get(b"get_host_plugin\0") } {
                Ok(symbol) => symbol,
                Err(error) => {
                    drop(lib);
                    let _ = std::fs::remove_file(&file_path);
                    return Err(SCPluginLoaderError::lib_error(error));
                }
            };
        let plugin = unsafe { get_host_plugin() };
        if plugin.abi_version != sc_plugin_api::HOST_PLUGIN_ABI_V1 {
            drop(lib);
            let _ = std::fs::remove_file(&file_path);
            return Err(SCPluginLoaderError::invalid_host_plugin_abi(
                plugin.abi_version,
            ));
        }
        unsafe {
            (plugin.enable)(plugin.context, api);
        }
        Ok(LoadedHostPlugin::new(plugin, lib, file_path))
    }
}
