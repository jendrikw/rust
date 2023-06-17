//! Optional checks for file types other than Rust source
//!
//! Handles python tool version managment via a virtual environment in
//! `build/venv`.

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Minimum python revision is 3.7 for ruff
// const MIN_PY_REV: (u32, u32) = (3, 7);
const MIN_PY_REV: (u32, u32) = (3, 7);
const MIN_PY_REV_STR: &str = "≥3.7";
const BLACK_VERSION: &str = "==23.3.0";
const RUFF_VERSION: &str = "==0.0.272";

/// Path to find the python executable within a virtual environment
#[cfg(target_os = "windows")]
const REL_PY_PATH: &[&str] = &["Scripts", "python3.exe"];
#[cfg(not(target_os = "windows"))]
const REL_PY_PATH: &[&str] = &["bin", "python3"];

pub fn check(
    root_path: &Path,
    outdir: &Path,
    bless: bool,
    extra_checks: Option<&str>,
    pos_args: &[String],
    bad: &mut bool,
) {
    if let Err(e) = check_impl(root_path, outdir, bless, extra_checks, pos_args) {
        tidy_error!(bad, "{e}");
    }
}

fn check_impl(
    root_path: &Path,
    outdir: &Path,
    bless: bool,
    extra_checks: Option<&str>,
    pos_args: &[String],
) -> Result<(), Error> {
    // Split comma-separated args up
    let lint_args = match extra_checks {
        Some(s) => s.strip_prefix("--extra-checks=").unwrap().split(',').collect(),
        None => vec![],
    };

    let mut py_lint = lint_args.contains(&"py:lint");
    let mut py_fmt = lint_args.contains(&"py:fmt");
    let mut shell_lint = lint_args.contains(&"shell:lint");
    let mut pip_checked = false;

    if lint_args.contains(&"py") {
        py_lint = true;
        py_fmt = true;
    }

    if lint_args.contains(&"shell") {
        shell_lint = true;
    }

    let mut py_path = None;

    if py_lint || py_fmt {
        let venv_path = outdir.join("venv");
        py_path = Some(get_or_create_venv(&venv_path)?);
    }

    if py_lint {
        eprintln!("linting python files");

        let mut cfg_path = root_path.to_owned();
        cfg_path.extend(["src", "tools", "tidy", "config", "ruff.toml"]);
        let mut args = vec![OsStr::new("--config"), cfg_path.as_ref()];
        add_pos_args(root_path, &mut args, pos_args);

        py_runner(py_path.as_ref().unwrap(), "ruff", RUFF_VERSION, &args, &mut pip_checked)?;
    }

    if py_fmt {
        let mut args = if bless {
            eprintln!("formatting python files");
            vec![]
        } else {
            eprintln!("checking python file format");
            vec![OsStr::new("--check")]
        };
        add_pos_args(root_path, &mut args, pos_args);

        py_runner(py_path.as_ref().unwrap(), "black", BLACK_VERSION, &args, &mut pip_checked)?;
    }

    if shell_lint {
        eprintln!("linting shell files");

        let mut args = vec![];
        let to_check;

        if pos_args.is_empty() {
            // shellcheck doesn't have a good file finder, so we need to help it
            to_check = find_with_extension(root_path, "sh")?;
            add_pos_args(root_path, &mut args, &to_check);
        } else {
            add_pos_args(root_path, &mut args, pos_args);
        }

        shellcheck_runner(&args)?;
    }

    Ok(())
}

/// If there are positional arguments, push them. Otherwise, just use the root path
fn add_pos_args<'a>(
    root_path: &'a Path,
    args: &mut Vec<&'a OsStr>,
    pos_args: &'a [impl AsRef<OsStr>],
) {
    args.push(OsStr::new("--"));
    if pos_args.is_empty() {
        args.push(root_path.as_os_str());
    } else {
        args.extend(pos_args.iter().map(AsRef::as_ref));
    }
}

/// Run a python command with given arguments. `py_path` should be a virtualenv.
fn py_runner(
    py_path: &Path,
    bin: &'static str,
    version: &str,
    args: &[&OsStr],
    pip_checked: &mut bool,
) -> Result<(), Error> {
    get_or_init_py_bin(py_path, bin, version, pip_checked)?;
    let status = Command::new(py_path).arg("-m").arg(bin).args(args).status()?;

    if status.success() { Ok(()) } else { Err(Error::FailedCheck(bin)) }
}

/// Create a virtuaenv at a given path if it doesn't already exist. Returns the
/// path to that venv's python executable.
fn get_or_create_venv(venv_path: &Path) -> Result<PathBuf, Error> {
    if !venv_path.is_dir() {
        create_venv_at_path(venv_path)?;
    }

    let mut py_path = venv_path.to_owned();
    py_path.extend(REL_PY_PATH);

    verify_py_version(&py_path)?;
    Ok(py_path)
}

/// Attempt to create a virtualenv at this path. Cycles through all expected
/// valid python versions to find one that is installed.
fn create_venv_at_path(path: &Path) -> Result<(), Error> {
    /// Preferred python versions in order. Newest to oldest then current
    /// development versions
    const TRY_PY: &[&str] = &[
        "python3.11",
        "python3.10",
        "python3.9",
        "python3.8",
        "python3.7",
        "python3",
        "python",
        "python3.12",
        "python3.13",
    ];

    let mut sys_py = None;
    let mut found = Vec::new();

    for py in TRY_PY {
        match verify_py_version(Path::new(py)) {
            Ok(_) => {
                sys_py = Some(*py);
                break;
            }
            // Skip not found errors
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => (),
            // Skip insufficient version errors
            Err(Error::Version { installed, .. }) => found.push(installed),
            // just log and skip unrecognized errors
            Err(e) => eprintln!("note: error running '{py}': {e}"),
        }
    }

    let Some(sys_py) = sys_py else {
        let ret = if found.is_empty() {
            Error::MissingReq("python3", "python file checks", None)
        } else{
            found.sort();
            found.dedup();
            Error::Version { program: "python3", required: MIN_PY_REV_STR, installed: found.join(", ") }
        };
        return Err(ret);
    };

    eprintln!("creating virtual environment at '{}' using '{sys_py}'", path.display());
    let out = Command::new(sys_py).args(["-m", "virtualenv"]).arg(path).output().unwrap();

    if out.status.success() {
        Ok(())
    } else if String::from_utf8_lossy(&out.stderr).contains("No module named virtualenv") {
        Err(Error::Generic(format!("virtualenv not found: is it installed for {sys_py}?")))
    } else {
        Err(Error::Generic(format!(
            "failed to create virtualenv at '{}' using {sys_py}",
            path.display()
        )))
    }
}

/// Parse python's version output (`Python x.y.z`) and ensure we have a
/// suitable version.
fn verify_py_version(py_path: &Path) -> Result<(), Error> {
    let out = Command::new(py_path).arg("--version").output()?;
    let outstr = String::from_utf8_lossy(&out.stdout);
    let vers = outstr.trim().split_ascii_whitespace().nth(1).unwrap().trim();
    let mut vers_comps = vers.split('.');
    let major: u32 = vers_comps.next().unwrap().parse().unwrap();
    let minor: u32 = vers_comps.next().unwrap().parse().unwrap();

    if (major < MIN_PY_REV.0) || ((major == MIN_PY_REV.0) && (minor < MIN_PY_REV.1)) {
        Err(Error::Version {
            program: "python",
            required: MIN_PY_REV_STR,
            installed: vers.to_owned(),
        })
    } else {
        Ok(())
    }
}

/// Given a binary (e.g. `black`, `ruff`) initialize it in the venv and return its path
fn get_or_init_py_bin(
    py_path: &Path,
    bin: &str,
    version: &str,
    pip_checked: &mut bool,
) -> Result<(), Error> {
    let bin_vers = format!("{bin}{version}");
    let out = Command::new(py_path).args(["-m", "pip", "freeze"]).output().unwrap().stdout;

    if String::from_utf8_lossy(&out).contains(&bin_vers) {
        return Ok(());
    }

    eprintln!("installing {bin_vers} via pip");

    if !*pip_checked {
        // verify pip is updated before installing anything
        Command::new(py_path)
            .args(["-m", "pip", "install", "--upgrade", "pip"])
            .status()
            .expect("failed to launch python");
        *pip_checked = true;
    }

    let status = Command::new(py_path).args(["-m", "pip", "install", &bin_vers]).status()?;
    if !status.success() {
        return Err(Error::Generic(format!("failed to install {bin}")));
    }

    Ok(())
}

/// Check that shellcheck is installed then run it at the given path
fn shellcheck_runner(args: &[&OsStr]) -> Result<(), Error> {
    match Command::new("shellcheck").arg("--version").status() {
        Ok(_) => (),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Error::MissingReq(
                "shellcheck",
                "shell file checks",
                Some(
                    "see <https://github.com/koalaman/shellcheck#installing> \
                    for installation instructions"
                        .to_owned(),
                ),
            ));
        }
        Err(e) => return Err(e.into()),
    }

    let status = Command::new("shellcheck").args(args).status()?;
    if status.success() { Ok(()) } else { Err(Error::FailedCheck("black")) }
}

/// Check git for tracked files matching an extension
fn find_with_extension(root_path: &Path, extension: &str) -> Result<Vec<PathBuf>, Error> {
    // Untracked files show up for short status and are indicated with a leading `?`
    // -C changes git to be as if run from that directory
    let stat_output =
        Command::new("git").arg("-C").arg(root_path).args(["status", "--short"]).output()?.stdout;

    if String::from_utf8_lossy(&stat_output).lines().filter(|ln| ln.starts_with('?')).count() > 0 {
        eprintln!("found untracked files, ignoring");
    }

    let mut output = Vec::new();
    let binding = Command::new("git").arg("-C").arg(root_path).args(["ls-files"]).output()?;
    let tracked = String::from_utf8_lossy(&binding.stdout);

    for line in tracked.lines() {
        let line = line.trim();
        let path = Path::new(line);

        if path.extension() == Some(OsStr::new(extension)) {
            output.push(path.to_owned());
        }
    }

    Ok(output)
}

#[derive(Debug)]
enum Error {
    Io(io::Error),
    /// a is required to run b. c is extra info
    MissingReq(&'static str, &'static str, Option<String>),
    /// Tool x failed the check
    FailedCheck(&'static str),
    /// Any message, just print it
    Generic(String),
    /// Installed but wrong version
    Version {
        program: &'static str,
        required: &'static str,
        installed: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingReq(a, b, ex) => {
                write!(
                    f,
                    "{a} is required to run {b} but it could not be located. Is it installed?"
                )?;
                if let Some(s) = ex {
                    write!(f, "\n{s}")?;
                };
                Ok(())
            }
            Self::Version { program, required, installed } => write!(
                f,
                "insufficient version of '{program}' to run external tools: \
                {required} required but found {installed}",
            ),
            Self::Generic(s) => f.write_str(s),
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::FailedCheck(s) => write!(f, "checks with external tool '{s}' failed"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
