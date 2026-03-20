use clap::{Parser, Subcommand};

/// Knowledge management subcommands matching KnowledgeService RPCs.
#[derive(Debug, Subcommand)]
pub enum KnowledgeArgs {
    /// Create a new knowledge document
    Create {
        /// Document title
        title: String,

        /// Project key (source = project name)
        #[arg(short, long, conflicts_with = "shared")]
        project: Option<String>,

        /// Store as shared knowledge (source = "shared")
        #[arg(long, conflicts_with = "project")]
        shared: bool,

        /// Short description (stored as first line of content)
        #[arg(short, long)]
        description: Option<String>,

        /// Body text (full content)
        #[arg(short, long, default_value = "")]
        body: String,

        /// Tags (can be repeated: -t foo -t bar)
        #[arg(short, long = "tag")]
        tags: Vec<String>,
    },

    /// Read a knowledge document by ID
    Read {
        /// Knowledge document ID
        id: String,
    },

    /// Update a knowledge document (partial update)
    Update {
        /// Knowledge document ID
        id: String,

        /// New title
        #[arg(long)]
        title: Option<String>,

        /// New description
        #[arg(short, long)]
        description: Option<String>,

        /// New body text
        #[arg(short, long)]
        body: Option<String>,

        /// Replace tags (can be repeated: --tag foo --tag bar)
        #[arg(short, long = "tag")]
        tags: Vec<String>,
    },

    /// Delete a knowledge document
    Delete {
        /// Knowledge document ID
        id: String,
    },

    /// List knowledge documents
    List {
        /// Project key (source = project name)
        #[arg(short, long, conflicts_with = "shared")]
        project: Option<String>,

        /// List shared knowledge (source = "shared")
        #[arg(long, conflicts_with = "project")]
        shared: bool,

        /// Filter by tag
        #[arg(short, long = "tag")]
        tag: Option<String>,
    },

    /// List distinct tags
    ListTags {
        /// Project key (source = project name)
        #[arg(short, long, conflicts_with = "shared")]
        project: Option<String>,

        /// List tags from shared knowledge (source = "shared")
        #[arg(long, conflicts_with = "project")]
        shared: bool,
    },
}

/// Wrapper struct for use as a clap subcommand group.
#[derive(Debug, Parser)]
pub struct KnowledgeCommand {
    #[command(subcommand)]
    pub command: KnowledgeArgs,
}
