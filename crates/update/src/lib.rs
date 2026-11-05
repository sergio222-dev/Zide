//! Self-update from GitHub Releases.
//!
//! The flow is: ask for the latest release, pick the archive for this platform, download it
//! with its minisign signature, check the signature against the public key embedded in the
//! binary, unpack the new executable and swap it in. Anything that does not verify is
//! discarded; with no key configured nothing is ever installed.

use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::Path,
};

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::Deserialize;

/// The public key releases are signed with, as written by `minisign -G`.
const PUBLIC_KEY: &str = include_str!("../zide.pub");

const MAX_RELEASE_JSON: usize = 1 << 20;
const MAX_ARCHIVE: usize = 200 << 20;
const MAX_SIGNATURE: usize = 4 << 10;
const MAX_BINARY: u64 = 400 << 20;

#[derive(Debug)]
pub enum Error {
    /// The embedded public key is missing or not a minisign key: updating is disabled.
    NoSigningKey,
    /// The platform has no release archives.
    Unsupported,
    Network(String),
    /// The release data is not what GitHub documents.
    Release(String),
    /// The signature does not match the archive: nothing was installed.
    BadSignature(String),
    Archive(String),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSigningKey => write!(f, "this build has no signing key, updates are disabled"),
            Self::Unsupported => write!(f, "no releases for this platform"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::Release(e) => write!(f, "unexpected release data: {e}"),
            Self::BadSignature(e) => write!(f, "signature check failed: {e}"),
            Self::Archive(e) => write!(f, "bad archive: {e}"),
            Self::Io(e) => write!(f, "cannot install: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Downloads a URL. A trait so the logic can be tested without a network.
pub trait Http {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, Error>;
}

/// The real thing.
pub struct Online;

impl Http for Online {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, Error> {
        let mut response = ureq::get(url)
            .header("User-Agent", "zide-updater")
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| Error::Network(e.to_string()))?;
        response
            .body_mut()
            .with_config()
            .limit(max_bytes as u64)
            .read_to_vec()
            .map_err(|e| Error::Network(e.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub assets: Vec<Asset>,
}

/// A newer release with everything needed to install it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: Version,
    pub archive: Asset,
    pub signature: Asset,
}

#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<AssetJson>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
}

/// The platform's release suffix, `None` where no archives are published.
pub fn current_target() -> Option<&'static str> {
    (cfg!(all(target_os = "linux", target_arch = "x86_64"))).then_some("x86_64-unknown-linux-gnu")
}

/// The latest published (not draft, not pre-release) release of `repo` (`owner/name`).
pub fn latest_release(http: &dyn Http, repo: &str) -> Result<Release, Error> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    parse_release(&http.get(&url, MAX_RELEASE_JSON)?)
}

fn parse_release(json: &[u8]) -> Result<Release, Error> {
    let release: ReleaseJson =
        serde_json::from_slice(json).map_err(|e| Error::Release(e.to_string()))?;
    if release.draft || release.prerelease {
        return Err(Error::Release("not a published release".into()));
    }
    let tag = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let version = Version::parse(tag).map_err(|e| Error::Release(format!("tag {tag}: {e}")))?;
    Ok(Release {
        version,
        assets: release
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
            })
            .collect(),
    })
}

/// Name of the archive of `version` for `target`.
pub fn archive_name(version: &Version, target: &str) -> String {
    format!("zide-{version}-{target}.tar.gz")
}

/// The update to install, when `release` is newer than `current` and has the archive of
/// `target` together with its signature.
pub fn find_update(current: &Version, release: &Release, target: &str) -> Option<Update> {
    if release.version <= *current {
        return None;
    }
    let name = archive_name(&release.version, target);
    let signature_name = format!("{name}.minisig");
    let asset = |wanted: &str| release.assets.iter().find(|a| a.name == wanted).cloned();
    Some(Update {
        version: release.version.clone(),
        archive: asset(&name)?,
        signature: asset(&signature_name)?,
    })
}

/// Whether a newer release exists for this platform.
pub fn check(http: &dyn Http, repo: &str, current: &Version) -> Result<Option<Update>, Error> {
    let target = current_target().ok_or(Error::Unsupported)?;
    public_key()?; // no point looking when nothing could be installed
    Ok(find_update(current, &latest_release(http, repo)?, target))
}

fn public_key() -> Result<PublicKey, Error> {
    public_key_from(PUBLIC_KEY)
}

fn public_key_from(text: &str) -> Result<PublicKey, Error> {
    PublicKey::decode(text).map_err(|_| Error::NoSigningKey)
}

/// Downloads the archive of `update` and returns it only if its signature verifies.
pub fn download_verified(
    http: &dyn Http,
    update: &Update,
    public_key: &str,
) -> Result<Vec<u8>, Error> {
    let key = public_key_from(public_key)?;
    let archive = http.get(&update.archive.url, MAX_ARCHIVE)?;
    let signature = http.get(&update.signature.url, MAX_SIGNATURE)?;
    let signature = std::str::from_utf8(&signature)
        .map_err(|_| Error::BadSignature("not text".into()))
        .and_then(|text| Signature::decode(text).map_err(|e| Error::BadSignature(e.to_string())))?;
    key.verify(&archive, &signature, false)
        .map_err(|e| Error::BadSignature(e.to_string()))?;
    Ok(archive)
}

/// The `zide` executable inside a release `.tar.gz`.
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>, Error> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let entries = tar.entries().map_err(|e| Error::Archive(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::Archive(e.to_string()))?;
        let is_binary = entry.header().entry_type().is_file()
            && entry
                .path()
                .is_ok_and(|p| p.file_name().is_some_and(|n| n == "zide"));
        if !is_binary {
            continue;
        }
        let mut binary = Vec::new();
        entry
            .take(MAX_BINARY)
            .read_to_end(&mut binary)
            .map_err(|e| Error::Archive(e.to_string()))?;
        return Ok(binary);
    }
    Err(Error::Archive("no `zide` executable inside".into()))
}

/// Replaces the executable at `exe` with `new`. The new file is written next to it and renamed
/// over it, so a crash leaves either the old or the new one, and a running process keeps its
/// own copy.
pub fn install(exe: &Path, new: &[u8]) -> Result<(), Error> {
    let staged = exe.with_extension("update");
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&staged)?;
        file.write_all(new)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        }
        fs::rename(&staged, exe)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    Ok(result?)
}

/// Downloads, verifies and installs `update` over the running executable.
pub fn apply(http: &dyn Http, update: &Update, exe: &Path) -> Result<Version, Error> {
    let archive = download_verified(http, update, PUBLIC_KEY)?;
    let binary = extract_binary(&archive)?;
    install(exe, &binary)?;
    Ok(update.version.clone())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, io::Cursor};

    use super::*;

    struct Fake(HashMap<String, Vec<u8>>);

    impl Http for Fake {
        fn get(&self, url: &str, _: usize) -> Result<Vec<u8>, Error> {
            self.0
                .get(url)
                .cloned()
                .ok_or_else(|| Error::Network(format!("404 {url}")))
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn release(version: &str, names: &[&str]) -> Release {
        Release {
            version: v(version),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: n.to_string(),
                    url: format!("https://dl/{n}"),
                })
                .collect(),
        }
    }

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    /// A key pair for tests: `(public key text, secret key)`.
    fn keys() -> (String, minisign::SecretKey) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        (pair.pk.to_box().unwrap().to_string(), pair.sk)
    }

    fn sign(secret: &minisign::SecretKey, data: &[u8]) -> Vec<u8> {
        minisign::sign(None, secret, Cursor::new(data), None, None)
            .unwrap()
            .to_string()
            .into_bytes()
    }

    fn update_for(version: &str) -> Update {
        find_update(
            &v("0.1.0"),
            &release(
                version,
                &[
                    &archive_name(&v(version), TARGET),
                    &format!("{}.minisig", archive_name(&v(version), TARGET)),
                ],
            ),
            TARGET,
        )
        .unwrap()
    }

    fn fake_with(update: &Update, archive: Vec<u8>, signature: Vec<u8>) -> Fake {
        Fake(HashMap::from([
            (update.archive.url.clone(), archive),
            (update.signature.url.clone(), signature),
        ]))
    }

    #[test]
    fn only_a_newer_release_with_both_assets_is_an_update() {
        let name = archive_name(&v("0.2.0"), TARGET);
        let signature = format!("{name}.minisig");
        let full = release("0.2.0", &[&name, &signature]);
        assert_eq!(
            find_update(&v("0.1.0"), &full, TARGET).unwrap().version,
            v("0.2.0")
        );
        assert!(
            find_update(&v("0.2.0"), &full, TARGET).is_none(),
            "same version"
        );
        assert!(
            find_update(&v("0.3.0"), &full, TARGET).is_none(),
            "older release"
        );
        assert!(find_update(&v("0.1.0"), &release("0.2.0", &[&name]), TARGET).is_none());
        assert!(find_update(&v("0.1.0"), &release("0.2.0", &[&signature]), TARGET).is_none());
        assert!(find_update(&v("0.1.0"), &full, "aarch64-apple-darwin").is_none());
    }

    #[test]
    fn release_json_is_parsed_and_drafts_are_refused() {
        let json = br#"{"tag_name":"v1.2.3","draft":false,"prerelease":false,
            "assets":[{"name":"a.tar.gz","browser_download_url":"https://x/a.tar.gz","size":1}]}"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, v("1.2.3"));
        assert_eq!(release.assets[0].name, "a.tar.gz");
        assert!(parse_release(br#"{"tag_name":"v1.0.0","draft":true,"assets":[]}"#).is_err());
        assert!(parse_release(br#"{"tag_name":"nightly","assets":[]}"#).is_err());
        assert!(parse_release(b"not json").is_err());
    }

    #[test]
    fn a_signed_archive_downloads_and_a_tampered_one_does_not() {
        let (public, secret) = keys();
        let update = update_for("0.2.0");
        let archive = tarball(&[("zide", b"binary")]);
        let signature = sign(&secret, &archive);

        let http = fake_with(&update, archive.clone(), signature.clone());
        assert_eq!(download_verified(&http, &update, &public).unwrap(), archive);

        let mut tampered = archive.clone();
        *tampered.last_mut().unwrap() ^= 1;
        let http = fake_with(&update, tampered, signature.clone());
        assert!(matches!(
            download_verified(&http, &update, &public),
            Err(Error::BadSignature(_))
        ));

        let (other_public, _) = keys();
        let http = fake_with(&update, archive, signature);
        assert!(matches!(
            download_verified(&http, &update, &other_public),
            Err(Error::BadSignature(_))
        ));
    }

    #[test]
    fn without_a_signing_key_nothing_verifies() {
        let update = update_for("0.2.0");
        let http = fake_with(&update, vec![1], vec![2]);
        assert!(matches!(
            download_verified(&http, &update, "no key here"),
            Err(Error::NoSigningKey)
        ));
        // the placeholder shipped in the repo is not a key either
        assert!(matches!(public_key(), Err(Error::NoSigningKey)));
    }

    #[test]
    fn the_binary_is_found_inside_the_archive() {
        let archive = tarball(&[("README", b"hi"), ("zide-0.2.0/zide", b"the binary")]);
        assert_eq!(extract_binary(&archive).unwrap(), b"the binary");
        assert!(matches!(
            extract_binary(&tarball(&[("README", b"hi")])),
            Err(Error::Archive(_))
        ));
        assert!(matches!(extract_binary(b"junk"), Err(Error::Archive(_))));
    }

    #[test]
    fn install_replaces_the_executable_and_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join(format!("zide-update-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("zide");
        fs::write(&exe, b"old").unwrap();
        install(&exe, b"new").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&exe).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no staged file left"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installing_into_a_missing_folder_fails_cleanly() {
        let exe = Path::new("/definitely/not/here/zide");
        assert!(matches!(install(exe, b"new"), Err(Error::Io(_))));
    }
}
