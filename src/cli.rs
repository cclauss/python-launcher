//! Parsing of CLI flags
//!
//! The [`Action`] enum represents what action to perform based on the
//! command-line arguments passed to the program.

use std::{
    collections::HashMap,
    env,
    fmt::Write,
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    str::FromStr,
    string::ToString,
};

use comfy_table::{Table, TableComponent};

use crate::{ExactVersion, RequestedVersion};

/// The expected directory name for virtual environments.
pub static DEFAULT_VENV_DIR: &str = ".venv";

/// Represents the possible outcomes based on CLI arguments.
#[derive(Clone, Debug, Hash, PartialEq)]
pub enum Action {
    /// The help string for the Python Launcher along with the path to a Python
    /// executable.
    ///
    /// The executable path is so that it can be executed with `-h` to append
    /// Python's own help output.
    Help(String, PathBuf),
    /// A string listing all found executables on `PATH`.
    ///
    /// The string is formatted to be human-readable.
    List(String),
    /// Details for executing a Python executable.
    Execute {
        /// The Python Launcher used to find the Python executable.
        launcher_path: PathBuf,
        /// The Python executable to run.
        executable: PathBuf,
        /// Arguments to the executable.
        args: Vec<String>,
    },
}

impl Action {
    /// Parses CLI arguments to determine what action should be taken.
    ///
    /// The first argument -- `argv[0]` -- is considered the path to the
    /// Launcher itself (i.e. [`Action::Execute::launcher_path`]).
    ///
    /// The second argument -- `argv.get(1)` -- is used to determine if/what
    /// argument has been provided for the Launcher.
    ///
    /// # Launcher Arguments
    ///
    /// ## `-h`/`--help`
    ///
    /// Returns [`Action::Help`].
    ///
    /// The search for the Python executable to use is done using
    /// [`crate::find_executable`] with an [`RequestedVersion::Any`] argument.
    ///
    /// ## `--list`
    ///
    /// Returns [`Action::List`].
    ///
    /// The list of executable is gathered via [`crate::all_executables`].
    ///
    /// ## Version Restriction
    ///
    /// Returns the appropriate [`Action::Execute`] instance for the requested
    /// Python version.
    ///
    /// [`crate::find_executable`] is used to perform the search.
    ///
    /// ## No Arguments for the Launcher
    ///
    /// Returns an [`Action::Execute`] instance.
    ///
    /// As a first step, a check is done for an activated virtual environment
    /// via the `VIRTUAL_ENV` environment variable. If none is set, look for a
    /// virtual environment in a directory named by [`DEFAULT_VENV_DIR`] in the
    /// current or any parent directories.
    ///
    /// If no virtual environment is found, a shebang line is searched for in
    /// the first argument to the Python interpreter. If one is found then it
    /// is used to (potentially) restrict the requested version searched for.
    ///
    /// The search for an interpreter proceeds using [`crate::find_executable`].
    ///
    /// # Errors
    ///
    /// If `-h`, `--help`, or `--list` are specified as the first argument but
    /// there are other arguments, [`crate::Error::IllegalArgument`] is returned.
    ///
    /// If no executable could be found for [`Action::Help`] or
    /// [`Action::List`], [`crate::Error::NoExecutableFound`] is returned.
    ///
    /// # Panics
    ///
    /// - If a [`writeln!`] call fails.
    /// - If the current directory cannot be accessed.
    pub fn from_main(argv: &[String]) -> crate::Result<Self> {
        let launcher_path = PathBuf::from(&argv[0]); // Strip the path to this executable.

        match argv.get(1) {
            Some(flag) if flag == "-h" || flag == "--help" || flag == "--list" => {
                if argv.len() > 2 {
                    Err(crate::Error::IllegalArgument(
                        launcher_path,
                        flag.to_string(),
                    ))
                } else if flag == "--list" {
                    Ok(Action::List(list_executables(&crate::all_executables())?))
                } else {
                    crate::find_executable(RequestedVersion::Any)
                        .ok_or(crate::Error::NoExecutableFound(RequestedVersion::Any))
                        .map(|executable_path| {
                            Action::Help(
                                help_message(&launcher_path, &executable_path),
                                executable_path,
                            )
                        })
                }
            }
            Some(version) if version_from_flag(version).is_some() => {
                Ok(Action::Execute {
                    launcher_path,
                    // Make sure to skip the app path and version specification.
                    executable: find_executable(version_from_flag(version).unwrap(), &argv[2..])?,
                    args: argv[2..].to_vec(),
                })
            }
            Some(_) | None => Ok(Action::Execute {
                launcher_path,
                // Make sure to skip the app path.
                executable: find_executable(RequestedVersion::Any, &argv[1..])?,
                args: argv[1..].to_vec(),
            }),
        }
    }
}

fn help_message(launcher_path: &Path, executable_path: &Path) -> String {
    let mut message = String::new();
    writeln!(
        message,
        include_str!("HELP.txt"),
        env!("CARGO_PKG_VERSION"),
        launcher_path.to_string_lossy(),
        executable_path.to_string_lossy()
    )
    .unwrap();
    message
}

/// Attempts to find a version specifier from a CLI argument.
///
/// It is assumed that the flag from the command-line is passed as-is
/// (i.e. the flag starts with `-`).
fn version_from_flag(arg: &str) -> Option<RequestedVersion> {
    if !arg.starts_with('-') {
        None
    } else {
        RequestedVersion::from_str(&arg[1..]).ok()
    }
}

fn list_executables(executables: &HashMap<ExactVersion, PathBuf>) -> crate::Result<String> {
    if executables.is_empty() {
        return Err(crate::Error::NoExecutableFound(RequestedVersion::Any));
    }

    let mut executable_pairs = Vec::from_iter(executables);
    executable_pairs.sort_unstable();
    executable_pairs.reverse();

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    // Using U+2502/"Box Drawings Light Vertical" over
    // U+007C/"Vertical Line"/pipe simply because it looks better.
    // Leaving out a header and other decorations to make it easier
    // parse the output.
    table.set_style(TableComponent::VerticalLines, '│');

    for (version, path) in executable_pairs {
        table.add_row(vec![version.to_string(), path.display().to_string()]);
    }

    Ok(table.to_string() + "\n")
}

fn relative_venv_path(add_default: bool) -> PathBuf {
    let mut path = PathBuf::new();
    if add_default {
        path.push(DEFAULT_VENV_DIR);
    }
    path.push("bin");
    path.push("python");
    path
}

/// Returns the path to the activated virtual environment's executable.
///
/// A virtual environment is determined to be activated based on the
/// existence of the `VIRTUAL_ENV` environment variable.
fn venv_executable_path(venv_root: &str) -> PathBuf {
    PathBuf::from(venv_root).join(relative_venv_path(false))
}

fn activated_venv() -> Option<PathBuf> {
    log::info!("Checking for VIRTUAL_ENV environment variable");
    env::var_os("VIRTUAL_ENV").map(|venv_root| {
        log::debug!("VIRTUAL_ENV set to {:?}", venv_root);
        venv_executable_path(&venv_root.to_string_lossy())
    })
}

fn venv_path_search() -> Option<PathBuf> {
    if env::current_dir().is_err() {
        log::warn!("current working directory is invalid");
        None
    } else {
        let cwd = env::current_dir().unwrap();
        log::info!(
            "Searching for a venv in {} and parent directories",
            cwd.display()
        );
        cwd.ancestors().find_map(|path| {
            let venv_path = path.join(relative_venv_path(true));
            log::info!("Checking {}", venv_path.display());
            // bool::then_some() makes more sense, but still experimental.
            venv_path.is_file().then(|| venv_path)
        })
    }
}

fn venv_executable() -> Option<PathBuf> {
    activated_venv().or_else(venv_path_search)
}

// https://en.m.wikipedia.org/wiki/Shebang_(Unix)
fn parse_python_shebang(reader: &mut impl Read) -> Option<RequestedVersion> {
    let mut shebang_buffer = [0; 2];
    log::info!("Looking for a Python-related shebang");
    if reader.read(&mut shebang_buffer).is_err() || shebang_buffer != [0x23, 0x21] {
        // Doesn't start w/ `#!` in ASCII/UTF-8.
        log::debug!("No '#!' at the start of the first line of the file");
        return None;
    }

    let mut buffered_reader = BufReader::new(reader);
    let mut first_line = String::new();

    if buffered_reader.read_line(&mut first_line).is_err() {
        log::debug!("Can't read first line of the file");
        return None;
    };

    // Whitespace between `#!` and the path is allowed.
    let line = first_line.trim();

    let accepted_paths = [
        "python",
        "/usr/bin/python",
        "/usr/local/bin/python",
        "/usr/bin/env python",
    ];

    for acceptable_path in &accepted_paths {
        if !line.starts_with(acceptable_path) {
            continue;
        }

        log::debug!("Found shebang: {}", acceptable_path);
        let version = line[acceptable_path.len()..].to_string();
        log::debug!("Found version: {}", version);
        return RequestedVersion::from_str(&version).ok();
    }

    None
}

fn find_executable(version: RequestedVersion, args: &[String]) -> crate::Result<PathBuf> {
    let mut requested_version = version;
    let mut chosen_path: Option<PathBuf> = None;

    if requested_version == RequestedVersion::Any {
        if let Some(venv_path) = venv_executable() {
            chosen_path = Some(venv_path);
        } else if !args.is_empty() {
            // Using the first argument because it's the simplest and sanest.
            // We can't use the last argument because that could actually be an argument
            // to the Python module being executed. This is the same reason we can't go
            // searching for the first/last file path that we find. The only safe way to
            // get the file path regardless of its position is to replicate Python's arg
            // parsing and that's a **lot** of work for little gain. Hence we only care
            // about the first argument.
            let possible_file = &args[0];
            log::info!("Checking {:?} for a shebang", possible_file);
            if let Ok(mut open_file) = File::open(possible_file) {
                if let Some(shebang_version) = parse_python_shebang(&mut open_file) {
                    requested_version = shebang_version;
                }
            }
        }
    }

    if chosen_path.is_none() {
        if let Some(env_var) = requested_version.env_var() {
            log::info!("Checking the {} environment variable", env_var);
            if let Ok(env_var_value) = env::var(&env_var) {
                if !env_var_value.is_empty() {
                    log::debug!("{} = '{}'", env_var, env_var_value);
                    let env_requested_version = RequestedVersion::from_str(&env_var_value)?;
                    requested_version = env_requested_version;
                }
            } else {
                log::info!("{} not set", env_var);
            };
        }

        if let Some(executable_path) = crate::find_executable(requested_version) {
            chosen_path = Some(executable_path);
        }
    }

    chosen_path.ok_or(crate::Error::NoExecutableFound(requested_version))
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(&["py".to_string(), "--help".to_string(), "--list".to_string()] => Err(crate::Error::IllegalArgument(PathBuf::from("py"), "--help".to_string())))]
    #[test_case(&["py".to_string(), "--list".to_string(), "--help".to_string()] => Err(crate::Error::IllegalArgument(PathBuf::from("py"), "--list".to_string())))]
    fn from_main_illegal_argument_tests(argv: &[String]) -> crate::Result<Action> {
        Action::from_main(argv)
    }

    #[test_case("-S" => None ; "unrecognized short flag is None")]
    #[test_case("--something" => None ; "unrecognized long flag is None")]
    #[test_case("-3" => Some(RequestedVersion::MajorOnly(3)) ; "major version")]
    #[test_case("-3.6" => Some(RequestedVersion::Exact(3, 6)) ; "Exact/major.minor")]
    #[test_case("-42.13" => Some(RequestedVersion::Exact(42, 13)) ; "double-digit major & minor versions")]
    #[test_case("-3.6.4" => None ; "version flag with micro version is None")]
    fn version_from_flag_tests(flag: &str) -> Option<RequestedVersion> {
        version_from_flag(flag)
    }

    #[test]
    fn test_help_message() {
        let launcher_path = "/some/path/to/launcher";
        let python_path = "/a/path/to/python";

        let help = help_message(&PathBuf::from(launcher_path), &PathBuf::from(python_path));
        assert!(help.contains(env!("CARGO_PKG_VERSION")));
        assert!(help.contains(launcher_path));
        assert!(help.contains(python_path));
    }

    #[test]
    fn test_list_executables() {
        let mut executables: HashMap<ExactVersion, PathBuf> = HashMap::new();

        assert_eq!(
            list_executables(&executables),
            Err(crate::Error::NoExecutableFound(RequestedVersion::Any))
        );

        let python27_path = "/path/to/2/7/python";
        executables.insert(
            ExactVersion { major: 2, minor: 7 },
            PathBuf::from(python27_path),
        );
        let python36_path = "/path/to/3/6/python";
        executables.insert(
            ExactVersion { major: 3, minor: 6 },
            PathBuf::from(python36_path),
        );
        let python37_path = "/path/to/3/7/python";
        executables.insert(
            ExactVersion { major: 3, minor: 7 },
            PathBuf::from(python37_path),
        );

        // Tests try not to make any guarantees about explicit formatting, just
        // that the interpreters are in descending order of version and the
        // interpreter version comes before the path (i.e. in column order).
        let executables_list = list_executables(&executables).unwrap();
        // No critical data is missing.
        assert!(executables_list.contains("2.7"));
        assert!(executables_list.contains(python27_path));
        assert!(executables_list.contains("3.6"));
        assert!(executables_list.contains(python36_path));
        assert!(executables_list.contains("3.7"));
        assert!(executables_list.contains(python37_path));

        // Interpreters listed in the expected order.
        assert!(executables_list.find("3.7").unwrap() < executables_list.find("3.6").unwrap());
        assert!(executables_list.find("3.6").unwrap() < executables_list.find("2.7").unwrap());

        // Columns are in the expected order.
        assert!(
            executables_list.find("3.6").unwrap() < executables_list.find(python36_path).unwrap()
        );
        assert!(
            executables_list.find("3.7").unwrap() < executables_list.find(python36_path).unwrap()
        );
    }

    #[test]
    fn test_venv_executable_path() {
        let venv_root = "/path/to/venv";
        assert_eq!(
            venv_executable_path(venv_root),
            PathBuf::from("/path/to/venv/bin/python")
        );
    }

    #[test_case("/usr/bin/python" => None ; "missing shebang comment")]
    #[test_case("# /usr/bin/python" => None ; "missing exclamation point")]
    #[test_case("! /usr/bin/python" => None ; "missing octothorpe")]
    #[test_case("#! /bin/sh" => None ; "non-Python shebang")]
    #[test_case("#! /usr/bin/env python" => Some(RequestedVersion::Any) ; "typical 'env python'")]
    #[test_case("#! /usr/bin/python" => Some(RequestedVersion::Any) ; "typical 'python'")]
    #[test_case("#! /usr/local/bin/python" => Some(RequestedVersion::Any) ; "/usr/local")]
    #[test_case("#! python" => Some(RequestedVersion::Any) ; "bare 'python'")]
    #[test_case("#! /usr/bin/env python3.7" => Some(RequestedVersion::Exact(3, 7)) ; "typical 'env python' with minor version")]
    #[test_case("#! /usr/bin/python3.7" => Some(RequestedVersion::Exact(3, 7)) ; "typical 'python' with minor version")]
    #[test_case("#! python3.7" => Some(RequestedVersion::Exact(3, 7)) ; "bare 'python' with minor version")]
    #[test_case("#!/usr/bin/python" => Some(RequestedVersion::Any) ; "no space between shebang and path")]
    fn parse_python_shebang_tests(shebang: &str) -> Option<RequestedVersion> {
        parse_python_shebang(&mut shebang.as_bytes())
    }

    #[test_case(&[0x23, 0x21, 0xc0, 0xaf] => None ; "invalid UTF-8")]
    fn parse_python_sheban_include_invalid_bytes_tests(
        mut shebang: &[u8],
    ) -> Option<RequestedVersion> {
        parse_python_shebang(&mut shebang)
    }
}
