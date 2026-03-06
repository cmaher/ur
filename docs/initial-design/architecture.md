# **Ur: Coding LLM Coordination Framework**

**Architecture & Design Specification**

## **1\. System Overview**

Ur is a native macOS coordination framework designed to manage, monitor, and assist ephemeral, containerized instances of Claude Code.  
It aims to provide maximum developer velocity and security by running as a strict **Native Monolith** on the host, communicating with isolated worker containers via strictly typed Unix Domain Sockets (UDS) using tarpc, and relying on standard OS-level primitives (tmux, macOS containers) for isolation and visualization.

### **Core Principles**

* **Execution First:** The foundation of the system is the secure, credential-less container execution engine. Tickets and state are built *on top* of this rock-solid execution layer.  
* **Zero-Parsing:** The system does not parse raw ANSI terminal streams or guess agent state based on regex. Agent state is explicitly signaled via Rust-native RPC.  
* **Air-gapped Credentials:** Worker containers have no access to host credentials (Git SSH keys, Jira tokens). All privileged operations are proxied back to the host monolith.  
* **Native Execution:** The Ur server runs directly on macOS. It does not run inside Docker, ensuring native access to filesystem operations and the container binary.

## **2\. Component Topology & Directory Structure**

The system is organized as a single Cargo workspace containing three primary Rust crates, alongside a directory for managing container definitions.

### **2.1. ROUGH Directory Structure**

Note that we will put things that can exist indpendently of one another in their own crates, and the ur host monolity / agent tools will zip them together. We are trying to maintain a logical separation of services, even though we will be running them together.

.  
├── Cargo.toml                  \# Workspace definition  
├── crates/  
│   ├── ur/                     \# 1\. The Host Monolith  
│   │   ├── Cargo.toml  
│   │   └── src/                \# Server, TUI, CLI, CozoDB integration  
│   ├── agent\_tools/            \# 2\. The Worker CLI  
│   │   ├── Cargo.toml  
│   │   └── src/                \# tarpc client, git proxy caller  
│   └── ur\_rpc/                 \# 3\. The Shared RPC Library  
│       ├── Cargo.toml  
│       └── src/                \# tarpc \#\[service\] traits and data types  
└── containers/  
    └── claude-worker/          \# Definition for the worker container  
        ├── build.sh            \# Script to build the macOS container image  
        ├── entrypoint.sh       \# Container PID 1 (starts tmux & agent)  
        └── config/             \# Base configurations (e.g., YOLO mode config)

### **2.2. The Ur Server (crates/ur)**

The host monolith running natively on macOS. It encapsulates:

* **The CLI / TUI:** The frontend interfaces for the human operator (built with clap and ratatui).  
* **State Management:** An embedded CozoDB instance for tracking the ticket DAG, agent status, and history.  
* **Orchestration Engine:** Async logic (via tokio) to provision sockets, invoke the macOS container CLI, and manage physical Git repository clones.  
* **The RPC Server:** A tarpc server listening on Unix Domain Sockets to handle requests from isolated containers.

### **2.3. The Worker CLI (crates/agent\_tools)**

A tiny, statically compiled Rust CLI binary injected into the worker container.

* Claude is configured to use this binary to communicate with the outside world.  
* It executes synchronously, blocking the container's terminal until the Ur Server responds to its RPC calls.

### **2.4. The Shared RPC Library (crates/ur\_rpc)**

A shared library crate imported by both ur and agent\_tools. It contains the tarpc trait definitions and shared data types (e.g., GitResponse), ensuring perfectly synced contracts without external build tools like protoc.

## **3\. The Communications Bridge**

The boundary between the untrusted agent container and the trusted host is a Unix Domain Socket (UDS) powered by tarpc.

### **The Setup Phase**

1. The Ur Server creates a unique socket on the host: /tmp/ur/sockets/agent\_\<id\>.sock.  
2. The Server launches the container, mounting the host socket to a known path inside the container: \-v /tmp/ur/sockets/agent\_\<id\>.sock:/var/run/ur.sock.  
3. The Server's tokio::net::UnixListener begins serving the tarpc service.

### **The Protocol Contract (ur\_rpc/src/lib.rs)**

Instead of Protobufs, Ur uses native Rust traits.  
use serde::{Deserialize, Serialize};

\#\[derive(Serialize, Deserialize, Debug)\]  
pub struct GitResponse {  
    pub exit\_code: i32,  
    pub stdout: String,  
    pub stderr: String,  
}

\#\[tarpc::service\]  
pub trait UrAgentBridge {  
    /// Blocks the agent\_tools CLI until a human responds via the Ur TUI  
    async fn ask\_human(process\_id: String, question: String) \-\> String;  
      
    /// Proxies a git command to be executed natively on the host  
    async fn exec\_git(process\_id: String, args: Vec\<String\>, working\_dir: String) \-\> GitResponse;  
      
    /// Explicitly updates the agent's state in CozoDB  
    async fn report\_status(process\_id: String, status: String);  
      
    /// Interacts with the CozoDB Ticket DAG  
    async fn ticket\_read(ticket\_id: String) \-\> String;  
    async fn ticket\_spawn(parent\_id: String, title: String, description: String) \-\> String;  
    async fn ticket\_note(ticket\_id: String, note: String);  
}

## **4\. User Interfaces**

The ur host binary provides two modes of interaction.

### **4.1. The TUI (Control Dashboard)**

Built using ratatui, the TUI is a persistent async dashboard that connects to the internal Ur state.

* **Views:** \* The DAG Viewer: Displays the dot-delimited ticket hierarchy (e.g., ic100.1).  
  * Fleet Status (Working, Blocked, Completed).  
  * **The Request Inbox:** A dedicated pane displaying pending ask\_human requests from agents. The user can select a request, type a response, and resolve the pending tarpc future to unblock the agent.

### **4.2. The CLI (Execution & Intervention)**

Used for direct command-and-control.  
ur \<SUBCOMMAND\>

SUBCOMMANDS:  
  server              Starts the Ur background daemon and CozoDB instance.  
  tui                 Launches the TUI dashboard.  
  ticket create       \<TITLE\> \[--parent \<ID\>\] \[--desc \<DESC\>\]  
  process launch      \<TICKET\_ID\> \[--role \<coder|reviewer|designer\>\]  
  process status      \[\<PROCESS\_ID\>\]  
    
  \# The Intervention Command  
  process attach      \<PROCESS\_ID\>

**The process attach mechanic:**  
For visual debugging, the CLI bypasses Ur's RPC logic. It executes the host's container exec binary as a subprocess and attaches standard I/O to the container's tmux session, dropping the human directly into Claude Code.

## **5\. Agent Interaction Workflows**

Claude Code (running inside tmux in the container) relies on the agent\_tools binary.

### **Workflow A: Asking for Help (Blocking)**

1. Claude runs: agent\_tools ask "Does the API prefer snake\_case or camelCase?"  
2. agent\_tools connects to /var/run/ur.sock and invokes the ask\_human RPC. **The CLI process blocks.**  
3. The Ur Server updates CozoDB state to Blocked\_On\_User.  
4. The TUI displays the prompt. The user provides an answer: "camelCase".  
5. The Ur Server completes the tarpc future.  
6. agent\_tools receives the string, prints "camelCase" to stdout, and exits 0\.  
7. Claude reads the output and resumes execution.

### **Workflow B: Executing Privileged Commands (Git)**

1. Claude runs: agent\_tools git push origin feature-branch  
2. agent\_tools captures the arguments and its current working directory, invoking exec\_git.  
3. The Ur Server receives the RPC. It translates the container's working directory to the absolute path of the repository clone on the macOS host.  
4. The Ur Server executes git push origin feature-branch using tokio::process::Command, securely utilizing the host's native network stack and SSH credentials.  
5. The Ur Server returns the GitResponse struct.  
6. agent\_tools prints the stdout/stderr, allowing Claude to verify the push succeeded.

## **6\. Roles & YOLO Mode**

Claude Code runs in absolute YOLO mode (auto-accepting file modifications and command executions). Safety is enforced by the absolute network and credential air-gap, while Context is enforced by Role configuration.

* **Designer:** Has explicit permission (via system prompt/instructions) to utilize agent\_tools ask frequently to clarify requirements before generating technical specifications or spawning sub-tickets.  
* **Coder / Reviewer:** Strongly discouraged from using agent\_tools ask unless a hard blocker is encountered. They are expected to rely on the provided CozoDB ticket context and execute independently.