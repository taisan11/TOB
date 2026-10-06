//! TypeScript plugin runtime backed by rustyscript and deno_core.

use anyhow::{Context, Result};
use rustyscript::{Module, Runtime};
use std::{fs, path::Path};

/// Owns a JavaScript/TypeScript runtime and loads `.ts`/`.js` plugin modules.
///
/// Plugins execute in a Deno/V8 runtime. Host APIs and packet hooks are deliberately
/// not exposed yet; adding them here keeps the proxy core independent of JS details.
pub struct PluginHost {
    runtime: Runtime,
    loaded: usize,
}

impl PluginHost {
    pub fn new() -> Result<Self> {
        Ok(Self {
            runtime: Runtime::new(Default::default()).context("create plugin runtime")?,
            loaded: 0,
        })
    }

    pub fn load_dir(&mut self, directory: &Path) -> Result<usize> {
        if !directory.exists() {
            log::info!(
                "plugin directory {} does not exist; skipping",
                directory.display()
            );
            return Ok(0);
        }
        let mut files = fs::read_dir(directory)
            .with_context(|| format!("read plugin directory {}", directory.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        files.sort_by_key(|entry| entry.path());

        for entry in files {
            let path = entry.path();
            if !path.is_file()
                || !matches!(
                    path.extension().and_then(|ext| ext.to_str()),
                    Some("ts" | "js")
                )
            {
                continue;
            }
            let source = fs::read_to_string(&path)
                .with_context(|| format!("read plugin {}", path.display()))?;
            let name = path.to_string_lossy().into_owned();
            self.runtime
                .load_module(&Module::new(name, source))
                .with_context(|| format!("evaluate plugin {}", path.display()))?;
            self.loaded += 1;
            log::info!("loaded plugin {}", path.display());
        }
        Ok(self.loaded)
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded
    }
}
