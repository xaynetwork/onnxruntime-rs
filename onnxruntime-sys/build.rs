#![allow(dead_code)]

use std::{
    borrow::Cow,
    env, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

/// ONNX Runtime version
///
/// WARNING: If version is changed, bindings for all platforms will have to be re-generated.
///          To do so, run this:
///              cargo build --package onnxruntime-sys --features generate-bindings
const ORT_VERSION: &str = "1.11.1";

/// Base Url from which to download pre-built releases/
const ORT_RELEASE_BASE_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download";

/// Environment variable selecting which strategy to use for finding the library
/// Possibilities:
/// * "download": Download a pre-built library from upstream. This is the default if `ORT_STRATEGY` is not set.
/// * "system": Use installed library. Use `ORT_LIB_LOCATION` to point to proper location.
/// * "compile": Download source and compile (TODO).
const ORT_ENV_STRATEGY: &str = "ORT_STRATEGY";

/// Name of environment variable that, if present, contains the location of a pre-built library.
/// Only used if `ORT_STRATEGY=system`.
const ORT_ENV_SYSTEM_LIB_LOCATION: &str = "ORT_LIB_LOCATION";
/// Name of environment variable that, if present, controls wether to use CUDA or not.
const ORT_ENV_GPU: &str = "ORT_USE_CUDA";

/// Subdirectory (of the 'target' directory) into which to extract the prebuilt library.
const ORT_PREBUILT_EXTRACT_DIR: &str = "onnxruntime";

// default for cargo ndk is 21 but for the build script it is 27
// both need to be the same otherwise linking might fail
const ANDROID_API_LEVEL: u32 = 27;

#[cfg(feature = "disable-sys-build-script")]
fn main() {
    println!("Build script disabled!");
}

#[cfg(not(feature = "disable-sys-build-script"))]
fn main() {
    let libort_install_dir = prepare_libort_dir();

    let include_dir = libort_install_dir.join("include");
    let lib_dir = libort_install_dir.join("lib");

    println!("Include directory: {:?}", include_dir);
    println!("Lib directory: {:?}", lib_dir);

    // Tell cargo to tell rustc to link onnxruntime shared library.
    println!("cargo:rustc-link-lib=onnxruntime");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    println!("cargo:rerun-if-env-changed={}", ORT_ENV_STRATEGY);
    println!("cargo:rerun-if-env-changed={}", ORT_ENV_GPU);
    println!("cargo:rerun-if-env-changed={}", ORT_ENV_SYSTEM_LIB_LOCATION);

    generate_bindings(&include_dir);
}

#[cfg(not(feature = "generate-bindings"))]
fn generate_bindings(_include_dir: &Path) {
    println!("Bindings not generated automatically, using committed files instead.");
    println!("Enable with the 'generate-bindings' cargo feature.");

    // NOTE: If bindings could not be be generated for Apple Sillicon M1, please uncomment the following
    // let os = env::var("CARGO_CFG_TARGET_OS").expect("Unable to get TARGET_OS");
    // let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Unable to get TARGET_ARCH");
    // if os == "macos" && arch == "aarch64" {
    //     panic!(
    //         "OnnxRuntime {} bindings for Apple M1 are not available",
    //         ORT_VERSION
    //     );
    // }
}

#[cfg(feature = "generate-bindings")]
fn generate_bindings(include_dir: &Path) {
    let mut clang_args = vec![
        format!("-I{}", include_dir.display()),
        format!(
            "-I{}",
            include_dir
                .join("onnxruntime")
                .join("core")
                .join("session")
                .display()
        ),
    ];

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=src/generated/bindings.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    match target_os.as_str() {
        "android" => {
            let ndk = env::var("ANDROID_NDK_HOME").expect("Failed to get ANDROID_NDK_HOME");

            #[cfg(target_os = "macos")]
            const HOST_ARCH: &str = "darwin-x86_64";
            #[cfg(target_os = "linux")]
            const HOST_ARCH: &str = "linux-x86_64";

            // copied from https://github.com/chertov/onnxruntime-rs/commit/4be4f7f20cc1e79fabc386e94b10466844cc1f99 to prevent errors like:
            // onnxruntime-rs/target/aarch64-linux-android/debug/build/onnxruntime-sys-80e652991449e5bd/out/onnxruntime-release/android-aarch64-v1.8.1/include/onnxruntime_c_api.h:5:10: fatal error: 'stdlib.h' file not found
            // we might not need this if we run it with cargo ndk

            let sysroot_dir = PathBuf::from(&ndk)
                .join("toolchains")
                .join("llvm")
                .join("prebuilt")
                .join(HOST_ARCH)
                .join("sysroot");

            let ndk_include = format!("{}/usr/include", sysroot_dir.display());
            let ndk_target = match target_arch.as_str() {
                "x86" => "i686-linux-android",
                "x86_64" => "x86_64-linux-android",
                "arm" => "arm-linux-androideabi",
                "aarch64" => "aarch64-linux-android",
                target => panic!("Unknown android target '{}'", target),
            };
            let ndk_target_include = format!("{}/{}", ndk_include, ndk_target);

            clang_args.push(format!("-I{}", ndk_include));
            clang_args.push(format!("-I{}", ndk_target_include));
        }
        _ => {}
    }

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header("wrapper.h")
        // The current working directory is 'onnxruntime-sys'
        .clang_args(clang_args)
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        // Set `size_t` to be translated to `usize` for win32 compatibility.
        .size_t_is_usize(true)
        // Format using rustfmt
        .rustfmt_bindings(true)
        .rustified_enum("*")
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to (source controlled) src/generated/<os>/<arch>/bindings.rs
    let generated_file = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("src")
        .join("generated")
        .join(env::var("CARGO_CFG_TARGET_OS").unwrap())
        .join(env::var("CARGO_CFG_TARGET_ARCH").unwrap())
        .join("bindings.rs");
    println!("cargo:rerun-if-changed={:?}", generated_file);
    bindings
        .write_to_file(&generated_file)
        .expect("Couldn't write bindings!");
}

fn download<P>(source_url: &str, target_file: P)
where
    P: AsRef<Path>,
{
    let resp = ureq::get(source_url)
        .timeout(std::time::Duration::from_secs(300))
        .call()
        .unwrap_or_else(|err| panic!("ERROR: Failed to download {}: {:?}", source_url, err));

    let len = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap();
    let mut reader = resp.into_reader();
    // FIXME: Save directly to the file
    let mut buffer = vec![];
    let read_len = reader.read_to_end(&mut buffer).unwrap();
    assert_eq!(buffer.len(), len);
    assert_eq!(buffer.len(), read_len);

    let f = fs::File::create(&target_file).unwrap();
    let mut writer = io::BufWriter::new(f);
    writer.write_all(&buffer).unwrap();
}

fn extract_archive(filename: &Path, output: &Path) {
    match filename.extension().map(|e| e.to_str()) {
        Some(Some("zip")) => extract_zip(filename, output),
        Some(Some("tgz")) => extract_tgz(filename, output),
        _ => unimplemented!(),
    }
}

fn extract_tgz(filename: &Path, output: &Path) {
    let file = fs::File::open(&filename).unwrap();
    let buf = io::BufReader::new(file);
    let tar = flate2::read::GzDecoder::new(buf);
    let mut archive = tar::Archive::new(tar);
    archive.unpack(output).unwrap();
}

fn extract_zip(filename: &Path, outpath: &Path) {
    let file = fs::File::open(&filename).unwrap();
    let buf = io::BufReader::new(file);
    let mut archive = zip::ZipArchive::new(buf).unwrap();
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        #[allow(deprecated)]
        let outpath = outpath.join(file.sanitized_name());
        if !(&*file.name()).ends_with('/') {
            println!(
                "File {} extracted to \"{}\" ({} bytes)",
                i,
                outpath.as_path().display(),
                file.size()
            );
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(&p).unwrap();
                }
            }
            let mut outfile = fs::File::create(&outpath).unwrap();
            io::copy(&mut file, &mut outfile).unwrap();
        }
    }
}

trait OnnxPrebuiltArchive {
    fn as_onnx_str(&self) -> Cow<str>;
}

#[derive(Debug)]
enum Architecture {
    X86,
    X86_64,
    Arm,
    Arm64,
}

impl FromStr for Architecture {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "x86" => Ok(Architecture::X86),
            "x86_64" => Ok(Architecture::X86_64),
            "arm" => Ok(Architecture::Arm),
            "aarch64" => Ok(Architecture::Arm64),
            _ => Err(format!("Unsupported architecture: {}", s)),
        }
    }
}

impl OnnxPrebuiltArchive for Architecture {
    fn as_onnx_str(&self) -> Cow<str> {
        match self {
            Architecture::X86 => Cow::from("x86"),
            Architecture::X86_64 => Cow::from("x64"),
            Architecture::Arm => Cow::from("arm"),
            Architecture::Arm64 => Cow::from("arm64"),
        }
    }
}

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
enum Os {
    Windows,
    Linux,
    MacOs,
}

impl Os {
    fn archive_extension(&self) -> &'static str {
        match self {
            Os::Windows => "zip",
            Os::Linux => "tgz",
            Os::MacOs => "tgz",
        }
    }
}

impl FromStr for Os {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "windows" => Ok(Os::Windows),
            "macos" => Ok(Os::MacOs),
            "linux" => Ok(Os::Linux),
            _ => Err(format!("Unsupported os: {}", s)),
        }
    }
}

impl OnnxPrebuiltArchive for Os {
    fn as_onnx_str(&self) -> Cow<str> {
        match self {
            Os::Windows => Cow::from("win"),
            Os::Linux => Cow::from("linux"),
            Os::MacOs => Cow::from("osx"),
        }
    }
}

#[derive(Debug)]
enum Accelerator {
    None,
    Gpu,
}

impl FromStr for Accelerator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "1" | "yes" | "true" | "on" => Ok(Accelerator::Gpu),
            _ => Ok(Accelerator::None),
        }
    }
}

impl OnnxPrebuiltArchive for Accelerator {
    fn as_onnx_str(&self) -> Cow<str> {
        match self {
            Accelerator::None => Cow::from(""),
            Accelerator::Gpu => Cow::from("gpu"),
        }
    }
}

#[derive(Debug)]
struct Triplet {
    os: Os,
    arch: Architecture,
    accelerator: Accelerator,
}

impl OnnxPrebuiltArchive for Triplet {
    fn as_onnx_str(&self) -> Cow<str> {
        match (&self.os, &self.arch, &self.accelerator) {
            // onnxruntime-win-x86-1.8.1.zip
            // onnxruntime-win-x64-1.8.1.zip
            // onnxruntime-win-arm-1.8.1.zip
            // onnxruntime-win-arm64-1.8.1.zip
            // onnxruntime-linux-x64-1.8.1.tgz
            // onnxruntime-osx-x64-1.8.1.tgz
            (Os::Windows, Architecture::X86, Accelerator::None)
            | (Os::Windows, Architecture::X86_64, Accelerator::None)
            | (Os::Windows, Architecture::Arm, Accelerator::None)
            | (Os::Windows, Architecture::Arm64, Accelerator::None)
            | (Os::Linux, Architecture::X86_64, Accelerator::None)
            | (Os::MacOs, Architecture::X86_64, Accelerator::None) => Cow::from(format!(
                "{}-{}",
                self.os.as_onnx_str(),
                self.arch.as_onnx_str()
            )),
            // onnxruntime-win-gpu-x64-1.8.1.zip
            // Note how this one is inverted from the linux one next
            (Os::Windows, Architecture::X86_64, Accelerator::Gpu) => Cow::from(format!(
                "{}-{}-{}",
                self.os.as_onnx_str(),
                self.accelerator.as_onnx_str(),
                self.arch.as_onnx_str(),
            )),
            // onnxruntime-linux-x64-gpu-1.8.1.tgz
            // Note how this one is inverted from the windows one above
            (Os::Linux, Architecture::X86_64, Accelerator::Gpu) => Cow::from(format!(
                "{}-{}-{}",
                self.os.as_onnx_str(),
                self.arch.as_onnx_str(),
                self.accelerator.as_onnx_str(),
            )),
            _ => {
                panic!(
                    "Unsupported prebuilt triplet: {:?}, {:?}, {:?}. Please use {}=system and {}=/path/to/onnxruntime",
                    self.os, self.arch, self.accelerator, ORT_ENV_STRATEGY, ORT_ENV_SYSTEM_LIB_LOCATION
                );
            }
        }
    }
}

fn prebuilt_archive_url() -> (PathBuf, String) {
    let triplet = Triplet {
        os: env::var("CARGO_CFG_TARGET_OS")
            .expect("Unable to get TARGET_OS")
            .parse()
            .unwrap(),
        arch: env::var("CARGO_CFG_TARGET_ARCH")
            .expect("Unable to get TARGET_ARCH")
            .parse()
            .unwrap(),
        accelerator: env::var(ORT_ENV_GPU).unwrap_or_default().parse().unwrap(),
    };

    let prebuilt_archive = format!(
        "onnxruntime-{}-{}.{}",
        triplet.as_onnx_str(),
        ORT_VERSION,
        triplet.os.archive_extension()
    );
    let prebuilt_url = format!(
        "{}/v{}/{}",
        ORT_RELEASE_BASE_URL, ORT_VERSION, prebuilt_archive
    );

    (PathBuf::from(prebuilt_archive), prebuilt_url)
}

fn prepare_libort_dir_prebuilt() -> PathBuf {
    let (prebuilt_archive, prebuilt_url) = prebuilt_archive_url();

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let extract_dir = out_dir.join(ORT_PREBUILT_EXTRACT_DIR);
    let downloaded_file = out_dir.join(&prebuilt_archive);

    println!("cargo:rerun-if-changed={}", downloaded_file.display());

    if !downloaded_file.exists() {
        println!("Creating directory {:?}", out_dir);
        fs::create_dir_all(&out_dir).unwrap();

        println!(
            "Downloading {} into {}",
            prebuilt_url,
            downloaded_file.display()
        );
        download(&prebuilt_url, &downloaded_file);
    }

    if !extract_dir.exists() {
        println!("Extracting to {}...", extract_dir.display());
        extract_archive(&downloaded_file, &extract_dir);
    }

    extract_dir.join(prebuilt_archive.file_stem().unwrap())
}

fn prepare_libort_dir() -> PathBuf {
    let strategy = env::var(ORT_ENV_STRATEGY);
    println!(
        "strategy: {:?}",
        strategy
            .as_ref()
            .map(String::as_str)
            .unwrap_or_else(|_| "unknown")
    );

    let os = env::var("CARGO_CFG_TARGET_OS").expect("Failed to get TARGET_OS");

    match strategy.as_ref().map(String::as_str) {
        Err(_) => match os.as_str() {
            "android" => android::setup(),
            _ => prepare_libort_dir_prebuilt(),
        },
        Ok("download") => prepare_libort_dir_prebuilt(),
        Ok("system") => PathBuf::from(match env::var(ORT_ENV_SYSTEM_LIB_LOCATION) {
            Ok(p) => p,
            Err(e) => {
                panic!(
                    "Could not get value of environment variable {:?}: {:?}",
                    ORT_ENV_SYSTEM_LIB_LOCATION, e
                );
            }
        }),
        Ok("compile") => unimplemented!(),
        _ => panic!("Unknown value for {:?}", ORT_ENV_STRATEGY),
    }
}

pub mod android {
    use std::{env, fs, io::Read, path::PathBuf, process::Command, str::FromStr};

    use git2::{ErrorCode, Repository};

    use crate::{ANDROID_API_LEVEL, ORT_PREBUILT_EXTRACT_DIR, ORT_VERSION};

    pub type GenericError = Box<dyn std::error::Error + Sync + Send + 'static>;

    struct Target {
        os: OS,
        arch: Arch,
    }

    pub fn setup() -> PathBuf {
        let os = env::var("CARGO_CFG_TARGET_OS")
            .map(|s| OS::from_str(&s))
            .expect("Failed to get TARGET_OS")
            .expect("Failed to get TARGET_OS");

        let arch = env::var("CARGO_CFG_TARGET_ARCH")
            .map(|s| Arch::from_str(&s))
            .expect("Failed to get TARGET_ARCH")
            .expect("Failed to get TARGET_ARCH");

        let target = Target { os, arch };
        download_prebuilt(&target).unwrap_or_else(|_| compile(&target))
    }

    fn compile(target: &Target) -> PathBuf {
        let workdir = prepare_git_repository();
        build_target(&workdir, target)
    }

    fn download_prebuilt(target: &Target) -> Result<PathBuf, GenericError> {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let workdir = out_dir
            .join(format!("{}-download", ORT_PREBUILT_EXTRACT_DIR))
            .join(version_name(&target.os, &target.arch));
        let lib_dir = workdir.join("lib");
        let lib = lib_dir.join("libonnxruntime.so");

        if !lib.exists() {
            let base = "http://s3-de-central.profitbricks.com/xayn-yellow-bert/onnxruntime";
            let url = format!(
                "{}/{}/libonnxruntime.so",
                base,
                version_name(&target.os, &target.arch)
            );

            let mut resp = ureq::get(&url).call()?.into_reader();
            let mut buf = Vec::new();
            resp.read_to_end(&mut buf)
                .expect("Failed to read the content of the request");

            fs::create_dir_all(&lib_dir).expect("Failed to create library directory");
            fs::write(lib, buf).expect("Failed to write content into file");
        }

        Ok(workdir)
    }

    fn prepare_git_repository() -> PathBuf {
        const REPO_URL: &str = "https://github.com/microsoft/onnxruntime";

        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let workdir = out_dir.join(format!("{}-git", ORT_PREBUILT_EXTRACT_DIR));

        let repo = match Repository::clone(REPO_URL, &workdir) {
            Ok(repo) => repo,
            Err(err) if err.code() == ErrorCode::Exists => {
                Repository::open(&workdir).expect("Failed to open repository")
            }
            Err(err) => panic!("Failed to clone repository: {:?}", err),
        };

        let (object, reference) = repo
            .revparse_ext(&format!("v{}", ORT_VERSION))
            .expect("Object not found");

        repo.checkout_tree(&object, None)
            .expect("Failed to checkout");

        match reference {
            // gref is an actual reference like branches or tags
            Some(gref) => repo.set_head(gref.name().unwrap()),
            // this is a commit, not a reference
            None => repo.set_head_detached(object.id()),
        }
        .expect("Failed to set HEAD");

        repo.workdir()
            .expect("Failed to get working directory of repository")
            .into()
    }

    fn build_target(workdir: &PathBuf, target: &Target) -> PathBuf {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        let release_dir = out_dir.join(format!("{}-release", ORT_PREBUILT_EXTRACT_DIR));
        let version_dir = release_dir.join(version_name(&target.os, &target.arch));

        build_android(workdir, &version_dir, &target.arch);
        version_dir
    }

    fn build_android(workdir: &PathBuf, version_dir: &PathBuf, arch: &Arch) {
        let sdk = env::var("ANDROID_SDK_HOME").expect("Failed to get ANDROID_SDK_HOME");
        let ndk = env::var("ANDROID_NDK_HOME").expect("Failed to get ANDROID_NDK_HOME");
        let with_nnapi = env::var("ANDROID_NNAPI").ok();
        if with_nnapi.is_some() && ANDROID_API_LEVEL < 27 {
            panic!("nnapi requires no less than api level 27")
        }

        if !version_dir.exists() {
            let mut cmd = Command::new("sh");
            cmd.current_dir(&workdir)
                .arg("build.sh")
                .arg("--android")
                .arg("--android_sdk_path")
                .arg(sdk)
                .arg("--android_ndk_path")
                .arg(ndk)
                .arg("--android_abi")
                .arg(arch.as_android_arch())
                .arg("--android_api")
                .arg(ANDROID_API_LEVEL.to_string());

            if with_nnapi.is_some() {
                cmd.arg("--use_nnapi");
            }

            cmd.arg("--parallel")
                .arg("0")
                // don't run x84_64 tests on android emulator
                .arg("--skip_tests")
                .arg("--build_shared_lib")
                .arg("--config")
                .arg("Release");

            let status = cmd.status().expect("Process failed to execute");
            if !status.success() {
                panic!(
                    "Failed to build android library for target {}",
                    arch.to_string()
                )
            }
            mimic_release_package(workdir, version_dir, &OS::Android, with_nnapi.is_some());
        }
    }

    fn version_name(os: &OS, arch: &Arch) -> String {
        format!(
            "{}-{}-lvl-{}-v{}",
            os.to_string(),
            arch.to_string(),
            ANDROID_API_LEVEL,
            ORT_VERSION
        )
    }

    fn mimic_release_package(
        workdir: &PathBuf,
        version_dir: &PathBuf,
        os: &OS,
        with_extra_provider: bool,
    ) {
        fs::create_dir_all(version_dir).expect("Failed to create release directory");
        let build_dir = workdir
            .join("build")
            .join(os.as_build_name())
            .join("Release");

        let lib_target_dir = version_dir.join("lib");
        fs::create_dir(&lib_target_dir).unwrap();
        fs::copy(
            build_dir.join("libonnxruntime.so"),
            lib_target_dir.join("libonnxruntime.so"),
        )
        .unwrap();

        // https://github.com/microsoft/onnxruntime/blob/f2ca43fe0d6ab1156bb43128e76c283bd21e46c5/tools/ci_build/github/linux/copy_strip_binary.sh
        let include_target_dir = version_dir.join("include");
        fs::create_dir(&include_target_dir).unwrap();

        let include_source_base = workdir.join("include").join("onnxruntime").join("core");
        fs::copy(
            include_source_base
                .join("session")
                .join("onnxruntime_c_api.h"),
            include_target_dir.join("onnxruntime_c_api.h"),
        )
        .unwrap();
        fs::copy(
            include_source_base
                .join("session")
                .join("onnxruntime_cxx_api.h"),
            include_target_dir.join("onnxruntime_cxx_api.h"),
        )
        .unwrap();
        fs::copy(
            include_source_base
                .join("session")
                .join("onnxruntime_cxx_inline.h"),
            include_target_dir.join("onnxruntime_cxx_inline.h"),
        )
        .unwrap();
        fs::copy(
            include_source_base
                .join("providers")
                .join("cpu")
                .join("cpu_provider_factory.h"),
            include_target_dir.join("cpu_provider_factory.h"),
        )
        .unwrap();
        fs::copy(
            include_source_base
                .join("session")
                .join("onnxruntime_session_options_config_keys.h"),
            include_target_dir.join("onnxruntime_session_options_config_keys.h"),
        )
        .unwrap();
        fs::copy(
            include_source_base
                .join("session")
                .join("onnxruntime_run_options_config_keys.h"),
            include_target_dir.join("onnxruntime_run_options_config_keys.h"),
        )
        .unwrap();
        fs::copy(
            include_source_base
                .join("framework")
                .join("provider_options.h"),
            include_target_dir.join("provider_options.h"),
        )
        .unwrap();

        if with_extra_provider {
            match os {
                OS::Android => {
                    fs::copy(
                        include_source_base
                            .join("providers")
                            .join("nnapi")
                            .join("nnapi_provider_factory.h"),
                        include_target_dir.join("nnapi_provider_factory.h"),
                    )
                    .unwrap();
                }
            }
        }
    }

    #[allow(non_camel_case_types)]
    enum OS {
        Android,
    }

    impl ToString for OS {
        fn to_string(&self) -> String {
            match self {
                OS::Android => "android",
            }
            .to_string()
        }
    }

    impl FromStr for OS {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s {
                "android" => Ok(OS::Android),
                _ => Err(format!("unsupported os: {}", s)),
            }
        }
    }

    impl OS {
        fn as_build_name(&self) -> &str {
            match self {
                OS::Android => "Android",
            }
        }
    }

    #[allow(non_camel_case_types)]
    enum Arch {
        x86,
        x86_64,
        arm,
        aarch64,
    }

    impl ToString for Arch {
        fn to_string(&self) -> String {
            match &self {
                Arch::x86 => "x86",
                Arch::x86_64 => "x86_64",
                Arch::arm => "arm",
                Arch::aarch64 => "aarch64",
            }
            .to_string()
        }
    }

    impl FromStr for Arch {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s {
                "x86" => Ok(Arch::x86),
                "x86_64" => Ok(Arch::x86_64),
                "arm" => Ok(Arch::arm),
                "aarch64" => Ok(Arch::aarch64),
                _ => Err(format!("unsupported arch: {}", s)),
            }
        }
    }

    impl Arch {
        fn as_android_arch(&self) -> &str {
            match self {
                Arch::x86 => "x86",
                Arch::x86_64 => "x86_64",
                Arch::arm => "armeabi-v7a",
                Arch::aarch64 => "arm64-v8a",
            }
        }

        fn as_android_triple(&self) -> &str {
            match self {
                Arch::x86 => "i686-linux-android",
                Arch::x86_64 => "x86_64-linux-android",
                Arch::arm => "arm-linux-androideabi",
                Arch::aarch64 => "aarch64-linux-android",
            }
        }
    }
}
