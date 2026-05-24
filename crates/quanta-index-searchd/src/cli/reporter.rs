use std::io::{self, Write};

use crate::app::SearchdConfig;

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchdReporter;

impl SearchdReporter {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub fn write_bootstrap<W: Write>(
        self,
        writer: &mut W,
        config: &SearchdConfig,
    ) -> io::Result<()> {
        writeln!(writer, "quanta-index searchd")?;
        writeln!(writer, "state_root={}", config.state_root.display())?;
        writeln!(
            writer,
            "control_plane={}",
            config.control_plane_path.display()
        )?;
        writeln!(writer, "socket_path={}", config.socket_path.display())?;
        writeln!(writer, "transport=uds codec=cbor max_frame_bytes=16777216")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::SearchdReporter;
    use crate::app::SearchdConfig;

    #[test]
    fn writes_bootstrap_report_to_injected_writer() {
        let config = SearchdConfig::from_state_root(PathBuf::from("/tmp/quanta-index-state"));
        let mut output = Vec::new();

        let write_result = SearchdReporter::new().write_bootstrap(&mut output, &config);
        assert!(write_result.is_ok(), "write failed: {write_result:?}");

        let rendered_result = String::from_utf8(output);
        let rendered = match rendered_result {
            Ok(rendered) => rendered,
            Err(error) => {
                assert!(false, "utf-8 decode failed: {error}");
                return;
            }
        };
        assert!(rendered.contains("quanta-index searchd"));
        assert!(rendered.contains("state_root=/tmp/quanta-index-state"));
        assert!(rendered.contains("control-plane.sqlite3"));
        assert!(rendered.contains("searchd.sock"));
        assert!(rendered.contains("transport=uds"));
    }
}
