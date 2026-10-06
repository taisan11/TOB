use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
};
use tob_plugin::PluginHost;
use tokio::io::AsyncWriteExt;

const DOWNLOAD_LINKS_API: &str =
    "https://net-secondary.web.minecraft-services.net/api/v1.0/download/links";

#[derive(Parser)]
#[command(
    name = "tob",
    version,
    about = "Minecraft Bedrock NetherNet proxy server"
)]
struct Cli {
    #[arg(short, long, global = true, default_value = "tob.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download the official BDS, configure NetherNet, and create tob.toml.
    Setup {
        #[arg(long, default_value = "bds")]
        directory: PathBuf,
        #[arg(long, value_enum)]
        platform: Option<BdsPlatform>,
    },
    /// Start TOB, launch BDS, and proxy NetherNet signaling.
    Serve,
    /// Parse and validate configuration and BDS installation.
    Check,
    /// Launch BDS without the proxy (diagnostics).
    BdsStart,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum BdsPlatform {
    Linux,
    Windows,
}

impl BdsPlatform {
    fn download_type(self) -> &'static str {
        match self {
            Self::Linux => "serverBedrockLinux",
            Self::Windows => "serverBedrockWindows",
        }
    }

    fn executable(self, dir: &Path) -> PathBuf {
        dir.join(match self {
            Self::Linux => "bedrock_server",
            Self::Windows => "bedrock_server.exe",
        })
    }
}

#[derive(Debug, Deserialize)]
struct Config {
    proxy: ProxyConfig,
    #[serde(default = "default_plugin_dir")]
    plugin_dir: PathBuf,
    bds: BdsConfig,
}

#[derive(Debug, Deserialize)]
struct ProxyConfig {
    listen: std::net::SocketAddr,
    upstream: std::net::SocketAddr,
}

#[derive(Debug, Deserialize)]
struct BdsConfig {
    directory: PathBuf,
    platform: BdsPlatform,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Deserialize)]
struct DownloadResponse {
    result: DownloadResult,
}

#[derive(Deserialize)]
struct DownloadResult {
    links: Vec<DownloadLink>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadLink {
    download_type: String,
    download_url: String,
}

fn default_plugin_dir() -> PathBuf {
    "plugins".into()
}

fn read_config(path: &Path) -> Result<Config> {
    let source =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let config: Config =
        toml::from_str(&source).with_context(|| format!("parse config {}", path.display()))?;
    Ok(config)
}

async fn download_and_setup(
    directory: &Path,
    config_path: &Path,
    platform: BdsPlatform,
) -> Result<()> {
    if config_path.exists() {
        bail!(
            "{} already exists; move it or choose another --config path",
            config_path.display()
        );
    }
    fs::create_dir_all(directory)
        .with_context(|| format!("create BDS directory {}", directory.display()))?;
    let executable = platform.executable(directory);
    if !executable.exists() {
        let http = reqwest::Client::builder()
            .user_agent(concat!("TOB/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let links: DownloadResponse = http
            .get(DOWNLOAD_LINKS_API)
            .send()
            .await
            .context("request official Minecraft download manifest")?
            .error_for_status()
            .context("Minecraft download manifest returned an error")?
            .json()
            .await
            .context("decode Minecraft download manifest")?;
        let link = links
            .result
            .links
            .into_iter()
            .find(|entry| entry.download_type == platform.download_type())
            .context("official manifest has no download for this platform")?;
        let url =
            reqwest::Url::parse(&link.download_url).context("invalid official BDS download URL")?;
        if url.scheme() != "https" || url.host_str() != Some("www.minecraft.net") {
            bail!("refusing unexpected BDS download origin: {url}");
        }
        log::info!("downloading BDS from {url}");
        let response = http
            .get(url)
            .send()
            .await?
            .error_for_status()
            .context("BDS download failed")?;
        let archive_path = directory.join(".bedrock-server-download.zip");
        let mut archive = tokio::fs::File::create(&archive_path).await?;
        let mut response = response;
        while let Some(chunk) = response.chunk().await? {
            archive.write_all(&chunk).await?;
        }
        archive.flush().await?;
        drop(archive);
        extract_bds_zip(&archive_path, directory)?;
        fs::remove_file(&archive_path).ok();
    }
    if !executable.exists() {
        bail!(
            "BDS executable not found after extraction: {}",
            executable.display()
        );
    }

    configure_server_properties(&directory.join("server.properties"))?;
    let config_parent = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let bds_dir = path_for_config(directory, config_parent);
    let text = format!(
        "plugin_dir = \"plugins\"\n\n[proxy]\nlisten = \"0.0.0.0:19132\"\nupstream = \"127.0.0.1:19133\"\n\n[bds]\ndirectory = \"{}\"\nplatform = \"{}\"\nargs = []\n",
        bds_dir.display(),
        match platform {
            BdsPlatform::Linux => "linux",
            BdsPlatform::Windows => "windows",
        }
    );
    let mut config_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(config_path)
        .with_context(|| format!("create config {}", config_path.display()))?;
    config_file.write_all(text.as_bytes())?;
    log::info!(
        "installed BDS and configured NetherNet in {}",
        directory.display()
    );
    println!(
        "Setup complete. Start TOB and BDS with: tob --config {} serve",
        config_path.display()
    );
    Ok(())
}

fn path_for_config(path: &Path, config_parent: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let parent = if config_parent.is_absolute() {
        config_parent.to_owned()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(config_parent)
    };
    absolute
        .strip_prefix(&parent)
        .map(Path::to_owned)
        .unwrap_or(absolute)
}

fn extract_bds_zip(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive_path).context("open BDS archive")?;
    let mut zip = zip::ZipArchive::new(file).context("read BDS ZIP archive")?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).context("read ZIP entry")?;
        let Some(relative) = entry.enclosed_name() else {
            bail!("unsafe path in BDS archive");
        };
        let output = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out =
            File::create(&output).with_context(|| format!("extract {}", output.display()))?;
        io::copy(&mut entry, &mut out)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

fn configure_server_properties(path: &Path) -> Result<()> {
    let source = fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();
    set_property(&mut lines, "transport", "nethernet");
    set_property(&mut lines, "server-port", "19133");
    set_property(&mut lines, "server-portv6", "19134");
    set_property(&mut lines, "server-udp-ports", "19140-19155");
    set_property(&mut lines, "enable-lan-visibility", "false");
    fs::write(path, format!("{}\n", lines.join("\n")))
        .with_context(|| format!("write {}", path.display()))
}

fn set_property(lines: &mut Vec<String>, key: &str, value: &str) {
    let prefix = format!("{key}=");
    if let Some(line) = lines.iter_mut().find(|line| line.starts_with(&prefix)) {
        *line = format!("{key}={value}");
    } else {
        lines.push(format!("{key}={value}"));
    }
}

async fn launch_bds(config: &Config) -> Result<tokio::process::Child> {
    if host_platform() != Some(config.bds.platform) {
        bail!(
            "this BDS installation targets {:?}, but this host is {:?}",
            config.bds.platform,
            host_platform()
        );
    }
    let executable = config.bds.platform.executable(&config.bds.directory);
    if !executable.exists() {
        bail!(
            "BDS is not installed at {}; run `tob setup` first",
            executable.display()
        );
    }
    let mut command = tokio::process::Command::new(&executable);
    command
        .args(&config.bds.args)
        .current_dir(&config.bds.directory)
        .stdin(Stdio::null());
    #[cfg(unix)]
    {
        // BDS ships shared libraries beside the executable; its working directory is
        // already set to that directory above.
        let mut library_path = std::ffi::OsString::from(".");
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            library_path.push(":");
            library_path.push(existing);
        }
        command.env("LD_LIBRARY_PATH", library_path);
    }
    let child = command
        .spawn()
        .with_context(|| format!("start BDS: {}", executable.display()))?;
    log::info!("started BDS (pid {})", child.id().unwrap_or_default());
    Ok(child)
}

async fn stop_bds(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    if let Command::Setup {
        directory,
        platform,
    } = cli.command
    {
        let platform = platform.or_else(host_platform).context("BDS officially supports Linux x86_64 and Windows x86_64; specify --platform on a supported host")?;
        return download_and_setup(&directory, &cli.config, platform).await;
    }
    let config = read_config(&cli.config)?;
    match cli.command {
        Command::Check => {
            let executable = config.bds.platform.executable(&config.bds.directory);
            println!(
                "Configuration is valid; BDS executable {}",
                if executable.exists() {
                    "is installed"
                } else {
                    "is missing (run setup)"
                }
            );
        }
        Command::BdsStart => {
            let mut child = launch_bds(&config).await?;
            tokio::select! {
                status = child.wait() => { let status = status.context("wait for BDS")?; if !status.success() { bail!("BDS exited with {status}"); } }
                signal = tokio::signal::ctrl_c() => { signal?; stop_bds(&mut child).await; }
            }
        }
        Command::Serve => {
            let mut plugins = PluginHost::new()?;
            plugins.load_dir(&config.plugin_dir)?;
            log::info!("loaded {} plugin(s)", plugins.loaded_count());
            let mut child = launch_bds(&config).await?;
            if let Err(error) = tob_core::wait_for_upstream(config.proxy.upstream).await {
                stop_bds(&mut child).await;
                return Err(error);
            }
            let proxy = tob_core::run_nethernet_proxy(config.proxy.listen, config.proxy.upstream);
            tokio::pin!(proxy);
            tokio::select! {
                result = &mut proxy => { stop_bds(&mut child).await; result?; }
                status = child.wait() => { bail!("BDS exited unexpectedly: {}", status?); }
                signal = tokio::signal::ctrl_c() => { signal?; stop_bds(&mut child).await; }
            }
        }
        Command::Setup { .. } => unreachable!(),
    }
    Ok(())
}

fn host_platform() -> Option<BdsPlatform> {
    if std::env::consts::ARCH != "x86_64" {
        return None;
    }
    match std::env::consts::OS {
        "linux" => Some(BdsPlatform::Linux),
        "windows" => Some(BdsPlatform::Windows),
        _ => None,
    }
}
