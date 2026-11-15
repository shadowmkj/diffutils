// This file is part of the uutils diffutils package.
//
// For the full copyright and license information, please view the LICENSE-*
// files that was distributed with this source code.

use crate::params::Params;
use crate::side_diff;
use crate::utils::report_failure_to_read_input_file;
use std::env::ArgsOs;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{stdout, Write};
use std::iter::Peekable;
use std::process::ExitCode;

const BINARY_PROBE_LEN: usize = 16 * 1024;

fn usage_string(executable: &OsStr) -> String {
    format!("Usage: {} <from> <to>", executable.to_string_lossy())
}

fn is_binary(content: &[u8]) -> bool {
    content[..content.len().min(BINARY_PROBE_LEN)].contains(&0)
}

/// parses command line arguments for `sdiff`.
/// this is necessary since the options differ for `diff` and `sdiff` in GNU
/// for example: `-w` sets the columns in `sdiff` but in `diff` it ignores whitespace
/// https://www.gnu.org/software/diffutils/manual/html_node/diff-Options.html
/// https://www.gnu.org/software/diffutils/manual/html_node/sdiff-Options.html
pub fn parse_params<I: Iterator<Item = OsString>>(mut opts: Peekable<I>) -> Result<Params, String> {
    let executable = opts
        .next()
        .ok_or_else(|| "Usage: sdiff <from> <to>".to_string())?;
    let mut params = Params {
        executable,
        ..Default::default()
    };

    let mut operands = Vec::new();

    while let Some(param) = opts.next() {
        let param_str = param.to_string_lossy();
        if param == "--" {
            operands.extend(opts);
            break;
        }

        if param == "-s" || param == "--suppress-common-lines" {
            params.suppress_common_lines = true;
            continue;
        }

        if param == "-l" || param == "--left-column" {
            params.left_column = true;
            continue;
        }

        if param == "-t" || param == "--expand-tabs" {
            params.expand_tabs = true;
            continue;
        }

        if param == "-a" || param == "--text" {
            params.text = true;
            continue;
        }

        if param == "-w" || param == "--width" {
            let val = opts
                .next()
                .ok_or_else(|| format!("option '{param_str}' requires an argument"))?;
            let val_str = val.to_string_lossy();
            params.width = val_str
                .parse::<usize>()
                .map_err(|_| format!("invalid width '{val_str}'"))?;
            if params.width < 7 {
                return Err(format!("invalid width '{val_str}'"));
            }
            continue;
        }

        if let Some(rest) = param_str.strip_prefix("--width=") {
            params.width = rest
                .parse::<usize>()
                .map_err(|_| format!("invalid width '{rest}'"))?;
            if params.width < 7 {
                return Err(format!("invalid width '{rest}'"));
            }
            continue;
        }

        if param_str.starts_with("-w") && param_str.len() > 2 {
            let rest = &param_str[2..];
            params.width = rest
                .parse::<usize>()
                .map_err(|_| format!("invalid width '{rest}'"))?;
            if params.width < 7 {
                return Err(format!("invalid width '{rest}'"));
            }
            continue;
        }

        if param == "--tabsize" {
            let val = opts
                .next()
                .ok_or_else(|| "option '--tabsize' requires an argument".to_string())?;
            let val_str = val.to_string_lossy();
            params.tabsize = match val_str.parse::<usize>() {
                Ok(num) if num != 0 && num <= crate::params::MAX_TABSIZE => num,
                _ => {
                    return Err(format!("invalid tabsize '{val_str}'"));
                }
            };
            continue;
        }

        if let Some(rest) = param_str.strip_prefix("--tabsize=") {
            params.tabsize = match rest.parse::<usize>() {
                Ok(num) if num != 0 && num <= crate::params::MAX_TABSIZE => num,
                _ => {
                    return Err(format!("invalid tabsize '{rest}'"));
                }
            };
            continue;
        }

        if param_str.starts_with('-') && param_str != "-" {
            return Err(format!("unrecognized option '{param_str}'"));
        }

        operands.push(param);
    }

    if operands.len() != 2 {
        return Err(usage_string(&params.executable));
    }

    params.from = operands.remove(0);
    params.to = operands.remove(0);

    Ok(params)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Identical,
    Different,
    Trouble,
}

impl From<Outcome> for ExitCode {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Identical => Self::SUCCESS,
            Outcome::Different => Self::from(1),
            Outcome::Trouble => Self::from(2),
        }
    }
}

pub fn compare_files(params: &Params) -> Outcome {
    if same_file::is_same_file(&params.from, &params.to).unwrap_or(false) {
        if params.suppress_common_lines {
            return Outcome::Identical;
        }
        let content = match fs::read(&params.from) {
            Ok(c) => c,
            Err(e) => {
                report_failure_to_read_input_file(&params.executable, &params.from, &e);
                return Outcome::Trouble;
            }
        };
        if !params.text && is_binary(&content) {
            return Outcome::Identical;
        }
        let mut stdout = stdout().lock();
        side_diff::diff(&content, &content, &mut stdout, params);
        let _ = stdout.flush();
        return Outcome::Identical;
    }

    let mut io_error = false;
    let from_content = match fs::read(&params.from) {
        Ok(c) => c,
        Err(e) => {
            report_failure_to_read_input_file(&params.executable, &params.from, &e);
            io_error = true;
            vec![]
        }
    };

    let to_content = match fs::read(&params.to) {
        Ok(c) => c,
        Err(e) => {
            report_failure_to_read_input_file(&params.executable, &params.to, &e);
            io_error = true;
            vec![]
        }
    };

    if io_error {
        return Outcome::Trouble;
    }

    if !params.text && (is_binary(&from_content) || is_binary(&to_content)) {
        if from_content == to_content {
            return Outcome::Identical;
        }
        eprintln!(
            "Binary files {} and {} differ",
            params.from.to_string_lossy(),
            params.to.to_string_lossy()
        );
        return Outcome::Different;
    }

    let mut stdout = stdout().lock();
    let result = side_diff::diff(&from_content, &to_content, &mut stdout, params);
    let _ = stdout.flush();

    if !result.is_empty() {
        Outcome::Different
    } else {
        Outcome::Identical
    }
}

pub fn main(opts: Peekable<ArgsOs>) -> ExitCode {
    let params = match parse_params(opts) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    compare_files(&params).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::NamedTempFile;

    fn parse(args: &[&str]) -> Result<Params, String> {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        parse_params(os_args.into_iter().peekable())
    }

    #[test]
    fn test_parse_defaults() {
        let p = parse(&["sdiff", "file1", "file2"]).unwrap();
        assert_eq!(p.from, OsString::from("file1"));
        assert_eq!(p.to, OsString::from("file2"));
        assert_eq!(p.width, 130);
        assert_eq!(p.tabsize, 8);
        assert!(!p.left_column);
        assert!(!p.suppress_common_lines);
        assert!(!p.expand_tabs);
        assert!(!p.text);
    }

    #[test]
    fn test_parse_all_flags() {
        let p = parse(&[
            "sdiff",
            "-s",
            "-l",
            "-t",
            "-a",
            "-w",
            "80",
            "--tabsize=4",
            "f1",
            "f2",
        ])
        .unwrap();
        assert!(p.suppress_common_lines);
        assert!(p.left_column);
        assert!(p.expand_tabs);
        assert!(p.text);
        assert_eq!(p.width, 80);
        assert_eq!(p.tabsize, 4);
    }

    #[test]
    fn test_parse_attached_width() {
        let p1 = parse(&["sdiff", "--width=100", "f1", "f2"]).unwrap();
        assert_eq!(p1.width, 100);

        let p2 = parse(&["sdiff", "-w95", "f1", "f2"]).unwrap();
        assert_eq!(p2.width, 95);
    }

    #[test]
    fn test_invalid_width_rejected() {
        assert!(parse(&["sdiff", "-w", "6", "f1", "f2"]).is_err());
        assert!(parse(&["sdiff", "-w", "not_a_num", "f1", "f2"]).is_err());
        assert!(parse(&["sdiff", "-w"]).is_err());
        assert!(parse(&["sdiff", "--width"]).is_err());
    }

    #[test]
    fn test_tabsize_parsing() {
        let p1 = parse(&["sdiff", "--tabsize=4", "f1", "f2"]).unwrap();
        assert_eq!(p1.tabsize, 4);

        let p2 = parse(&["sdiff", "--tabsize", "2", "f1", "f2"]).unwrap();
        assert_eq!(p2.tabsize, 2);

        assert!(parse(&["sdiff", "--tabsize=0", "f1", "f2"]).is_err());
        assert!(parse(&["sdiff", "--tabsize", "0", "f1", "f2"]).is_err());
        assert!(parse(&["sdiff", "--tabsize=invalid", "f1", "f2"]).is_err());
        assert!(parse(&["sdiff", "--tabsize"]).is_err());
    }

    #[test]
    fn test_missing_operands() {
        assert!(parse(&["sdiff", "f1"]).is_err());
        assert!(parse(&["sdiff"]).is_err());
    }

    #[test]
    fn test_extra_operands() {
        assert!(parse(&["sdiff", "f1", "f2", "f3"]).is_err());
    }

    #[test]
    fn test_double_dash_operand_separator() {
        let p = parse(&["sdiff", "--", "-weird-file1", "-weird-file2"]).unwrap();
        assert_eq!(p.from, OsString::from("-weird-file1"));
        assert_eq!(p.to, OsString::from("-weird-file2"));
    }

    #[test]
    fn test_unrecognized_option() {
        let res = parse(&["sdiff", "-unknown", "f1", "f2"]);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("unrecognized option '-unknown'"));
    }

    #[test]
    fn test_outcome_exit_codes() {
        assert_eq!(ExitCode::from(Outcome::Identical), ExitCode::SUCCESS);
        assert_eq!(ExitCode::from(Outcome::Different), ExitCode::from(1));
        assert_eq!(ExitCode::from(Outcome::Trouble), ExitCode::from(2));
    }

    #[test]
    fn test_compare_files_identical() {
        let f1 = NamedTempFile::new().unwrap();
        fs::write(f1.path(), b"hello\nworld\n").unwrap();

        let params = Params {
            executable: OsString::from("sdiff"),
            from: f1.path().as_os_str().to_os_string(),
            to: f1.path().as_os_str().to_os_string(),
            ..Default::default()
        };

        assert_eq!(compare_files(&params), Outcome::Identical);
    }

    #[test]
    fn test_compare_files_different() {
        let f1 = NamedTempFile::new().unwrap();
        let f2 = NamedTempFile::new().unwrap();
        fs::write(f1.path(), b"hello\nleft\n").unwrap();
        fs::write(f2.path(), b"hello\nright\n").unwrap();

        let params = Params {
            executable: OsString::from("sdiff"),
            from: f1.path().as_os_str().to_os_string(),
            to: f2.path().as_os_str().to_os_string(),
            ..Default::default()
        };

        assert_eq!(compare_files(&params), Outcome::Different);
    }

    #[test]
    fn test_compare_files_missing() {
        let params = Params {
            executable: OsString::from("sdiff"),
            from: OsString::from("/path/to/nonexistent_file_a"),
            to: OsString::from("/path/to/nonexistent_file_b"),
            ..Default::default()
        };

        assert_eq!(compare_files(&params), Outcome::Trouble);
    }

    #[test]
    fn test_compare_files_binary_different() {
        let f1 = NamedTempFile::new().unwrap();
        let f2 = NamedTempFile::new().unwrap();
        fs::write(f1.path(), b"\0binary1").unwrap();
        fs::write(f2.path(), b"\0binary2").unwrap();

        let params = Params {
            executable: OsString::from("sdiff"),
            from: f1.path().as_os_str().to_os_string(),
            to: f2.path().as_os_str().to_os_string(),
            ..Default::default()
        };

        assert_eq!(compare_files(&params), Outcome::Different);
    }
}
