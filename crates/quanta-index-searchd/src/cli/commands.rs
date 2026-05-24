use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

/// Operator overrides accepted by `searchd serve`.
///
/// All fields are optional; missing fields fall back to the environment-driven
/// configuration computed in [`crate::app::SearchdConfig::from_env`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServeOptions {
    pub state_root: Option<PathBuf>,
    pub socket_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SearchdCommand {
    Serve(ServeOptions),
}

impl SearchdCommand {
    pub fn from_env() -> Result<Self> {
        Self::parse_args(std::env::args().skip(1))
    }

    pub fn parse_args<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut iter = args.into_iter();
        let head = iter.next().map(|arg| arg.as_ref().to_owned());
        let first_flag = match head.as_deref() {
            None | Some("serve") => None,
            Some(arg) if arg.starts_with("--") => Some(arg.to_owned()),
            Some(other) => bail!("unsupported subcommand: {other}"),
        };

        let mut options = ServeOptions::default();
        let mut pending: Option<String> = first_flag;
        loop {
            let token = match pending.take() {
                Some(t) => t,
                None => match iter.next() {
                    Some(t) => t.as_ref().to_owned(),
                    None => break,
                },
            };
            match token.as_str() {
                "--state-root" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow!("--state-root requires a value"))?;
                    options.state_root = Some(PathBuf::from(value.as_ref()));
                }
                "--socket-path" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| anyhow!("--socket-path requires a value"))?;
                    options.socket_path = Some(PathBuf::from(value.as_ref()));
                }
                flag if flag.starts_with("--") => {
                    bail!("unknown flag: {flag}");
                }
                other => {
                    bail!("unexpected positional argument: {other}");
                }
            }
        }
        Ok(Self::Serve(options))
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchdCommand, ServeOptions};
    use std::path::PathBuf;

    macro_rules! ok_or_fail {
        ($expr:expr, $msg:expr) => {
            match $expr {
                Ok(v) => v,
                Err(error) => {
                    assert!(false, "{}: {error}", $msg);
                    return;
                }
            }
        };
    }

    fn expect_serve(command: SearchdCommand) -> ServeOptions {
        match command {
            SearchdCommand::Serve(opts) => opts,
        }
    }

    #[test]
    fn defaults_to_serve_when_no_subcommand_is_present() {
        let command = ok_or_fail!(SearchdCommand::parse_args(Vec::<String>::new()), "parse");
        assert_eq!(expect_serve(command), ServeOptions::default());
    }

    #[test]
    fn accepts_explicit_serve_subcommand() {
        let command = ok_or_fail!(SearchdCommand::parse_args(["serve"]), "parse");
        assert_eq!(expect_serve(command), ServeOptions::default());
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let result = SearchdCommand::parse_args(["inspect"]);
        assert!(result.is_err(), "unexpected result: {result:?}");
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("unsupported subcommand")),
        );
    }

    #[test]
    fn parses_state_root_override() {
        let command = ok_or_fail!(
            SearchdCommand::parse_args(["serve", "--state-root", "/tmp/sr"]),
            "parse"
        );
        let options = expect_serve(command);
        assert_eq!(options.state_root, Some(PathBuf::from("/tmp/sr")));
        assert!(options.socket_path.is_none());
    }

    #[test]
    fn parses_socket_path_override() {
        let command = ok_or_fail!(
            SearchdCommand::parse_args(["serve", "--socket-path", "/tmp/s.sock"]),
            "parse"
        );
        let options = expect_serve(command);
        assert_eq!(options.socket_path, Some(PathBuf::from("/tmp/s.sock")));
    }

    #[test]
    fn flags_without_serve_keyword_default_to_serve() {
        let command = ok_or_fail!(
            SearchdCommand::parse_args(["--state-root", "/tmp/sr"]),
            "parse"
        );
        let options = expect_serve(command);
        assert_eq!(options.state_root, Some(PathBuf::from("/tmp/sr")));
    }

    #[test]
    fn rejects_unknown_flag() {
        let result = SearchdCommand::parse_args(["serve", "--bogus"]);
        assert!(result.is_err(), "unexpected: {result:?}");
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("unknown flag")),
        );
    }

    #[test]
    fn rejects_flag_without_value() {
        let result = SearchdCommand::parse_args(["serve", "--state-root"]);
        assert!(result.is_err());
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("requires a value")),
        );
    }

    #[test]
    fn parses_state_root_and_socket_path_together() {
        let command = ok_or_fail!(
            SearchdCommand::parse_args([
                "serve",
                "--state-root",
                "/tmp/sr",
                "--socket-path",
                "/tmp/s.sock",
            ]),
            "parse"
        );
        let options = expect_serve(command);
        assert_eq!(options.state_root, Some(PathBuf::from("/tmp/sr")));
        assert_eq!(options.socket_path, Some(PathBuf::from("/tmp/s.sock")));
    }
}
