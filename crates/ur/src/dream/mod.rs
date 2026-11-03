//! `ur dream`: the mechanical half of the `brain:dream` skill.
//!
//! Everything project-specific comes from the config block in `<brain>/DREAM.md`; this module
//! knows no project. Output is JSON (pretty in text mode, enveloped with `--output json`).
//! Only `check --write` and `reviewed` write, and only `<brain>/.dream/state.json`.
//!
//! Workers reach these commands through hostexec (`ur.lua` pins the worker's project), so the
//! brain path and check commands resolve on the host.

mod check;
mod config;
mod files;
mod lint;
mod refs;
mod review;
mod state;

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
    /// Run check commands for active streams' refs (refs cached as done are skipped)
    Check {
        #[command(flatten)]
        target: Target,
        /// Merge results into <brain>/.dream/state.json
        #[arg(long)]
        write: bool,
    },
    /// Parse TEXT as a ref of KIND and run its check once (for testing a ref kind)
    Try {
        #[command(flatten)]
        target: Target,
        kind: String,
        text: String,
    },
    /// Durable docs by review due date; `pick` is what to review this run
    Review(Target),
    /// Record a review of FILE (brain-relative) and adapt its review interval
    Reviewed {
        #[command(flatten)]
        target: Target,
        file: String,
        outcome: Outcome,
        /// What the review found or changed
        #[arg(long)]
        note: Option<String>,
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
        DreamCommands::Check { target, write } => {
            let brain = existing_brain(config, target)?;
            let cfg = Config::load(&brain)?;
            let mut st = State::load(&brain)?;
            let scanned = refs::scan(&brain, &cfg);
            let results = check::check_all(&cfg, &scanned.active_refs, &st);
            if write {
                check::record(&mut st, &results);
                st.save(&brain)?;
            }
            emit(
                output,
                &check::summarize(&results, &check::attribution(&scanned)),
            )
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
            file,
            outcome,
            note,
        } => {
            let brain = existing_brain(config, target)?;
            let cfg = Config::load(&brain)?;
            let mut st = State::load(&brain)?;
            let rec = review::record(
                &brain,
                &cfg,
                &mut st,
                file.trim_start_matches("./"),
                outcome,
                note,
            )?;
            st.save(&brain)?;
            emit(output, &rec)
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
