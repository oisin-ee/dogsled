//! Host-local derived metrics and their shared report boundary.
mod harvest;
mod index;
pub(crate) mod report;
mod store;

#[cfg(test)]
mod tests;

use portal_types::metrics::{MetricsReport, RunGroup, ToolGroup, Window};
use serde::Deserialize;

use crate::error::Result;
use crate::ledger::Ledger;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Options {
    pub window: Window,
    pub group: ToolGroup,
    pub run_group: RunGroup,
    pub tool: Option<String>,
}

impl Options {
    pub(crate) fn parse(
        window: &str,
        group: &str,
        run_group: &str,
        tool: Option<&str>,
    ) -> Result<Self> {
        Ok(serde_json::from_value(
            serde_json::json!({"window":window,"group":group,"run_group":run_group,"tool":tool}),
        )?)
    }

    pub(crate) fn argv(&self) -> Result<Vec<String>> {
        let name = |value: serde_json::Value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| crate::error::Error::Invalid("invalid metrics option".into()))
        };
        let mut args = vec![
            "metrics".into(),
            "--json".into(),
            "--window".into(),
            name(serde_json::to_value(self.window)?)?,
            "--group".into(),
            name(serde_json::to_value(self.group)?)?,
            "--run-group".into(),
            name(serde_json::to_value(self.run_group)?)?,
        ];
        if let Some(tool) = &self.tool {
            args.extend(["--tool".into(), tool.clone()]);
        }
        Ok(args)
    }
}

pub(crate) fn lane(
    ledger: &Ledger,
    home: &str,
    published: &crate::observation::Published,
) -> Result<()> {
    let reports: Vec<crate::hosts::HostReport> =
        crate::observation::read::view(published, "hosts")?;
    pass(ledger, home, &reports)
}

fn pass(ledger: &Ledger, home: &str, reports: &[crate::hosts::HostReport]) -> Result<()> {
    let source = harvest::Source::read(ledger)?;
    let mut connection = store::open(ledger)?;
    let now = crate::ledger::now_ms();
    index::pass(
        &mut connection,
        ledger,
        home,
        &source.bindings(reports),
        now,
    )?;
    source.write(
        &mut connection,
        &crate::shell::System {
            budget: ledger.budget,
        },
    )?;
    store::retain(&mut connection, now)?;
    store::write(&mut connection, |tx| {
        tx.execute("INSERT INTO index_state(id,indexed_ms) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET indexed_ms=excluded.indexed_ms", [crate::rows::integer(crate::ledger::now_ms())?])?;
        Ok(())
    })
}

pub(crate) fn local(ledger: &Ledger, options: &Options) -> Result<MetricsReport> {
    report::local(&mut store::open(ledger)?, options, crate::ledger::now_ms())
}
