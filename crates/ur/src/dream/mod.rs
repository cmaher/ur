//! `ur dream`: the mechanical half of the `brain:dream` skill.
//!
//! Everything project-specific comes from the config block in `<brain>/DREAM.md`; this module
//! knows no project. Output is JSON (pretty in text mode, enveloped with `--output json`).
//! Only `check`, `harvest` and `reviewed` write, and only `<brain>/.dream/state.json`.
//!
//! Workers reach these commands through hostexec (`ur.lua` pins the worker's project), so the
//! brain path and check commands resolve on the host.

mod candidates;
mod changes;
mod check;
mod config;
mod files;
mod harvest;
mod lint;
mod refs;
mod report;
mod review;
mod state;
mod status;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::output::OutputManager;
use config::Config;
use state::{Outcome, State};

#[derive(Args)]
pub struct Target {
    /// Project key (default: UR_PROJECT, then the current directory name)
    #[arg(short = 'p', long = "project")]
    project: Option<String>,
}

#[derive(Subcommand)]
pub enum DreamCommands {
    /// Print the project's brain directory
    Path(Target),
    /// Validate the DREAM.md config block and print it
    Config(Target),
    /// Broken links, index gaps, orphans, long index entries, empty and unlisted folders
    Lint(Target),
    /// Scratch streams: status, recorded and discovered refs, last change
    Refs(Target),
    /// Run check commands for active streams' refs (refs cached as done are skipped) and
    /// record the results in <brain>/.dream/state.json
    Check(Target),
    /// Classify active streams for promotion from cached checks (candidate, blocked,
    /// possibly_abandoned, active); never runs checks
    Candidates(Target),
    /// Where the last run stopped: newest report and its run commit, uncommitted paths
    /// (`.dream/` vs other), and its "Needs a human" items
    Status(Target),
    /// New messages from idle agent transcripts and changed memory files named by `[harvest]`;
    /// records what was returned so the next call returns only what is new
    Harvest(Target),
    /// Parse TEXT as a ref of KIND and run its check once (for testing a ref kind)
    Try {
        #[command(flatten)]
        target: Target,
        kind: String,
        text: String,
    },
    /// Durable docs by review due date; `pick` is what to review this run, `changes_since` what
    /// changed in each repo since a picked doc's last `reviewed --at`
    Review(Target),
    /// Record a review of each FILE (brain-relative) and adapt its review interval
    Reviewed {
        #[command(flatten)]
        target: Target,
        #[arg(required = true, num_args = 1..)]
        files: Vec<String>,
        outcome: Outcome,
        /// What the review found or changed
        #[arg(long)]
        note: Option<String>,
        /// REPO=SHA: the commit of a DREAM.md repo the docs were checked against (repeatable)
        #[arg(long = "at", value_name = "REPO=SHA", value_parser = changes::parse_at)]
        at: Vec<(String, String)>,
    },
}

pub fn handle(
    cmd: DreamCommands,
    config: &ur_config::Config,
    output: &OutputManager,
) -> Result<()> {
    match cmd {
        DreamCommands::Path(t) => {
            let brain = brain_path(config, t)?;
            emit(output, &serde_json::json!({ "brain": brain }))
        }
        DreamCommands::Config(t) => {
            let brain = existing_brain(config, t)?;
            emit(output, &Config::load(&brain)?.raw)
        }
        DreamCommands::Lint(t) => {
            let brain = existing_brain(config, t)?;
            emit(output, &lint::lint(&brain, &Config::load(&brain)?))
        }
        DreamCommands::Refs(t) => {
            let brain = existing_brain(config, t)?;
            emit(output, &refs::scan(&brain, &Config::load(&brain)?))
        }
        DreamCommands::Check(t) => {
            let brain = existing_brain(config, t)?;
            let cfg = Config::load(&brain)?;
            let mut st = State::load(&brain)?;
            let scanned = refs::scan(&brain, &cfg);
            let results = check::check_all(&cfg, &scanned.active_refs, &st);
            check::record(&mut st, &results);
            st.save(&brain)?;
            emit(
                output,
                &check::summarize(&results, &check::attribution(&scanned)),
            )
        }
        DreamCommands::Candidates(t) => {
            let brain = existing_brain(config, t)?;
            let cfg = Config::load(&brain)?;
            let scanned = refs::scan(&brain, &cfg);
            let st = State::load(&brain)?;
            emit(
                output,
                &candidates::classify(&scanned, &st, cfg.raw.thresholds.quiet_days),
            )
        }
        DreamCommands::Status(t) => {
            let brain = existing_brain(config, t)?;
            emit(output, &status::status(&brain)?)
        }
        DreamCommands::Harvest(t) => {
            let brain = existing_brain(config, t)?;
            let cfg = Config::load(&brain)?;
            let mut st = State::load(&brain)?;
            let report = harvest::harvest(&brain, &cfg, &st, harvest::default_as_of(&cfg));
            harvest::record(&mut st, &report);
            st.save(&brain)?;
            emit(output, &report)
        }
        DreamCommands::Try { target, kind, text } => {
            let brain = existing_brain(config, target)?;
            let cfg = Config::load(&brain)?;
            let r = refs::parse_ref(&cfg, &kind, &text);
            let (state, done) = check::run_one(&cfg, &r);
            let argv = check::argv(&cfg, &r);
            emit(
                output,
                &serde_json::json!({ "ref": r, "argv": argv, "state": state, "done": done }),
            )
        }
        DreamCommands::Review(t) => {
            let brain = existing_brain(config, t)?;
            let cfg = Config::load(&brain)?;
            emit(
                output,
                &review::schedule(&brain, &cfg, &State::load(&brain)?),
            )
        }
        DreamCommands::Reviewed {
            target,
            files,
            outcome,
            note,
            at,
        } => {
            let brain = existing_brain(config, target)?;
            let cfg = Config::load(&brain)?;
            let mut st = State::load(&brain)?;
            let finding = review::Finding {
                outcome,
                note,
                at: changes::resolve_at(&brain, &cfg, &at)?,
            };
            let files: Vec<String> = files
                .iter()
                .map(|f| f.trim_start_matches("./").to_string())
                .collect();
            let recs = review::record_all(&brain, &cfg, &mut st, &files, &finding)?;
            st.save(&brain)?;
            emit(output, &recs)
        }
    }
}

fn brain_path(config: &ur_config::Config, target: Target) -> Result<PathBuf> {
    let key = ur_config::resolve_project(target.project, &config.projects)
        .context("no project: pass -p <project>")?;
    let project = config
        .projects
        .get(&key)
        .with_context(|| format!("unknown project '{key}'"))?;
    project
        .brain_path(&config.config_dir)
        .with_context(|| format!("project '{key}': brain_dir does not resolve to a host path"))
}

fn existing_brain(config: &ur_config::Config, target: Target) -> Result<PathBuf> {
    let brain = brain_path(config, target)?;
    if !brain.join(".git").exists() {
        bail!(
            "{} is not a git repository: run brain:init",
            brain.display()
        );
    }
    Ok(brain)
}

fn emit<T: Serialize>(output: &OutputManager, value: &T) -> Result<()> {
    if output.is_json() {
        output.print_success(value);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        cmd: DreamCommands,
    }

    #[test]
    fn reviewed_takes_several_files_then_outcome_and_repeated_at() {
        let cli = Cli::try_parse_from([
            "x", "reviewed", "a.md", "b.md", "changed", "--at", "r=abc", "--at", "s=def",
        ])
        .unwrap();
        let DreamCommands::Reviewed {
            files, outcome, at, ..
        } = cli.cmd
        else {
            panic!("not reviewed");
        };
        assert_eq!(files, vec!["a.md", "b.md"]);
        assert_eq!(outcome, Outcome::Changed);
        assert_eq!(
            at,
            vec![("r".into(), "abc".into()), ("s".into(), "def".into())]
        );
        assert!(Cli::try_parse_from(["x", "reviewed", "a.md", "changed", "--at", "r"]).is_err());
        assert!(Cli::try_parse_from(["x", "reviewed", "changed"]).is_err());
    }
}
