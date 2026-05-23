use anyhow::{Result, bail};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchdCommand {
    Serve,
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
        let mut args = args.into_iter();

        match args.next().map(|arg| arg.as_ref().to_owned()) {
            None => Ok(Self::Serve),
            Some(command) if command == "serve" => Ok(Self::Serve),
            Some(command) => bail!("unsupported subcommand: {command}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SearchdCommand;

    #[test]
    fn defaults_to_serve_when_no_subcommand_is_present() {
        let command_result = SearchdCommand::parse_args(Vec::<String>::new());
        let command = match command_result {
            Ok(command) => command,
            Err(error) => {
                assert!(false, "parse failed: {error}");
                return;
            }
        };
        assert_eq!(command, SearchdCommand::Serve);
    }

    #[test]
    fn accepts_explicit_serve_subcommand() {
        let command_result = SearchdCommand::parse_args(["serve"]);
        let command = match command_result {
            Ok(command) => command,
            Err(error) => {
                assert!(false, "parse failed: {error}");
                return;
            }
        };
        assert_eq!(command, SearchdCommand::Serve);
    }

    #[test]
    fn rejects_unknown_subcommand() {
        let result = SearchdCommand::parse_args(["inspect"]);
        assert!(result.is_err(), "unexpected result: {result:?}");
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("unsupported subcommand"))
        );
    }
}
