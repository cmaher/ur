# **Ur: Initial Epics**

This document outlines the chronological implementation plan for the Ur framework. It is organized into Epics and Tasks, utilizing the dot-delimited ticket ID schema (ur1, ur1.1, etc.). This backlog is strictly ordered to tackle the highest technical risks first: establishing the containerized execution environment and UDS communications bridge using tarpc, *then* building the stateful ticket DAG and TUI around it.

Epics 1 and 2 are of the HIGHEST PRIORITY so we can start dogfooding this thing

## **Epic ur1: Execution Engine & UDS Bridge**

**Goal:** Establish the Rust workspace, launch a macOS container, start tmux, and prove the Unix Domain Socket (UDS) tarpc bridge between the host and the isolated worker.

* **ur1.1 \- Workspace Initialization \[CLI/Server/Container\]:** Setup the Cargo workspace containing the main ur monolith crate, the agent\_tools CLI crate, and a shared ur\_rpc library crate. Include basic clap CLI structures.  
* **ur1.2 \- tarpc Definitions & Listener \[Server/Container\]:** Define the \#\[tarpc::service\] traits (UrAgentBridge) in the shared library. Implement a tokio::net::UnixListener in the Ur monolith that serves the tarpc endpoints over a domain socket.  
* **ur1.3 \- Container Launcher \[Server\]:** Implement the Rust wrapper around the macOS container CLI using std::process::Command. Must handle mounting a test host directory, mounting the UDS to /var/run/ur.sock, and executing the tmux entrypoint.  
* **ur1.4 \- Base agent\_tools Client \[Container\]:** Implement the skeletal agent\_tools CLI that connects to /var/run/ur.sock via tokio::net::UnixStream and a tarpc client. Successfully send a ping/status report to the server and exit. Bake this into the worker container image.  
* **ur1.5 \- CLI process attach \[CLI\]:** Implement the ur process attach \<ID\> command to execute container exec \-it \<id\> tmux attach \-t agent via the host CLI, allowing visual debugging of the worker.

## **Epic ur2: The Git Proxy & Credential Air-gap**

**Goal:** Give the isolated agent the ability to safely manipulate the host repository without mounting SSH keys into the container.

* **ur2.1 \- agent\_tools git Subcommand \[Container\]:** Implement the CLI argument parsing in agent\_tools to capture git commands and the current working directory, packaging them into the ExecGit tarpc call.  
* **ur2.2 \- Server-side Path Translation \[Server\]:** Implement the logic on the Ur Server to intercept ExecGit, map the container's virtual path to the physical host path of the cloned repo, and execute the command safely using tokio::process::Command.  
* **ur2.3 \- Git RPC Streaming \[Server/Container\]:** Ensure the stdout and stderr from the host's git execution is returned flawlessly back through the tarpc response so the agent sees the result exactly as if it ran it locally.

## **Epic ur3: Foundational State & Tickets**

**Goal:** Now that isolated execution is proven, embed the database and build the core ticket DAG engine to drive the work.

* **ur3.1 \- CozoDB Integration \[Server\]:** Embed CozoDB into the Ur server. Define the initial schema for Tickets (id, title, description, status, parent\_id).  
* **ur3.2 \- Ticket ID & DAG Logic \[Server\]:** Implement the custom ID generation logic (\<prefix\>\<num\>.\<sub\_num\>). Implement functions to compute the next available ID for child tickets and validate directed acyclic relationships.  
* **ur3.3 \- CLI Ticket Management \[CLI/Server\]:** Implement ur ticket create, ur ticket ls, and ur ticket show to allow human developers to build and view the local ticket graph.

## **Epic ur4: Human-in-the-Loop & TUI**

**Goal:** Build the interactive dashboard and implement the ask\_human blocking workflow so agents can request help.

* **ur4.1 \- TUI Scaffold \[CLI\]:** Initialize the ratatui interface within the Ur monolith. Implement basic layout (DAG Viewer on the left, Fleet Status on the top right, Inbox on the bottom right).  
* **ur4.2 \- Agent Blocking (ask\_human) \[Container/Server\]:** Implement agent\_tools ask \<question\>. It must await the AskHuman tarpc function, securely *blocking* the container process until a response is received. Update CozoDB state to Blocked\_On\_User.  
* **ur4.3 \- TUI Request Inbox \[CLI/Server\]:** Wire the TUI to display blocked agents in the Inbox. Implement a text input prompt allowing the user to type an answer, submit it, and complete the hanging tarpc future.  
* **ur4.4 \- Real-time Status Sync \[CLI/Server\]:** Ensure the TUI accurately reflects agent states (Provisioning, Running, Blocked, Completed) via CozoDB subscriptions or internal tokio::sync::broadcast channels.

## **Epic ur5: Agent Orchestration & Autonomy**

**Goal:** Connect the Tickets to the Agents, allowing Claude Code to independently read specs, execute work, and spawn sub-tasks.

* **ur5.1 \- Context Packaging \[Server\]:** When launching an agent for a ticket, automatically pull the ticket's markdown spec from CozoDB and generate a context.md file in the container workspace, or pass it via system prompts.  
* **ur5.2 \- Agent Ticket Tools \[Container/Server\]:** Implement agent\_tools ticket note, agent\_tools ticket status, and agent\_tools ticket spawn so the agent can read and write to the CozoDB ledger over the UDS.  
* **ur5.3 \- Dependency Scheduler \[Server\]:** Implement a basic orchestration loop in the Server. If an Epic (ur100) has open sub-tickets (ur100.1, ur100.2), prevent an agent from completing ur100 until the children are resolved.  
* **ur5.4 \- E2E Autonomy Test \[CLI/Server/Container\]:** Run a full workflow where a Designer agent analyzes a dummy Epic, spawns three sub-tickets, and Coder agents execute and close them without host credential leaks.