# Ur ADE Design

## Background

Claude Code often asks to run complicated bash commands and python scripts that cannot be auto-approved, due to the flexibility of the commands. In some cases, such as python3, blanket permission must not be granted, because that essentially grants access to every command available on your computer. Claude Code's sandbox mode offers some alleviation here, but it is not complete.

While restricting Claude's permissions increases security, it slows development, and it would be highly desirable to run Claude with all permissions granted, while still maintaining the desired level of security.

## Proposal

Run Claude Code in a container with restricted network access. All communication with the host, including the execution of commands, is mediated by a server with strictly deterministic (non-agentic) code.

Ur Agentic Development Environment (Ur ADE) is a set of tools that interoperate to manage the lifecycle of coding agents, while running Claude Code in a container with all permissions granted.

The project consists of the following key components:

* A **Worker** container that runs Claude on an isolated docker network
* **workerd**, a daemon running on the worker container
* A **Server** that runs in docker, with access to the host network and the worker network and mediates between the worker and **builderd**
* **builderd**, a host daemon that serves as the Workers' "hands" on the host machine
* **ur**, a host CLI that primarily exists to send messages to the **server**
* **urui**, a TUI that communicates with the server
* **Squid**, acting as a forward proxy, that allows certain network requests from Claude

Importantly, only a specific set of commands are permitted to be executed, and those commands are passed through a set of Lua scripts to ensure that only specific subcommands and flags are permitted.

## Architecture

The Server is the biggest component of the design, acting as the mediator between the client/host processes and the worker processes. In addition to handling command-forwarding, it features a DAG-based ticketing system, a RAG system for dependency docs, as well as management of the worker nodes and automating the lifecycle of tickets.

It uses a SQLite database stored on the host, and Qdrant as its vector database.

### Starting the Server

The server is started by running `ur server start`, which builds a Docker Compose file based on configuration (such as ports and the allowed directories for mounting), which it uses to build the docker networks and start the various containers.

This CLI call additionally launches a detached builderd process, which stores its PID in the ur config directory.

### Launching a Worker

Workers can be launched either via the TUI or the **ur** command. Workers launch in the context of a **project**, which is configured with a git repository and optional container mounts.

#### Git Repository

The server maintains a cache of git repositories for every project. Before launching a worker, it chooses an available repo from the pool, updates it to the latest origin/main (or master), and cleans it. This directory is then mounted to the worker. It is the only directory the worker has access to, by default.

#### Credentials

When launching a worker, we read Claude credentials from the keychain (on macOS) and copy them into the worker containers. Each container has its own copy of the credentials. Claude Code rotates these credentials as it works.

No other credentials are passed to the worker.

## Network Isolation

Workers run on a docker network that does not have access to the host or the internet. All internet-based requests go through Squid, which only allows access to Anthropic domains and the specific GCP bucket that holds Claude Code releases.

The server only runs on localhost and is not exposed to LAN or the internet.

### Host Exec

The worker is given several "host-exec" command wrappers, that are simple scripts delegating to the **workertools** binary loaded on the worker. Host-exec commands submit requests to the server, essentially mimicking the form of `command args...`.

Each worker has a UUID granted to it by the server that it must present with requests to ensure that it does not try to submit commands on behalf of other workers.

On the server, Lua scripts examine each request and reject it if it does not meet the criteria established for that command. For most commands, only read-only actions are allowed, but for others where write actions are required, the commands are stripped of flags that try to escape the directory.

The server then forwards these commands to **builderd**, with the CWD set to the directory assigned to the individual worker. The server uses a different codepath for commands that it needs to execute on the host directly.

#### Supported Commands

| Command | Policy |
|---------|--------|
| `git` | Blocks `-C`, `--work-tree`, `--git-dir` to prevent directory escape |
| `gh` | Read-only commands plus PR commenting. Blocks `-C` flag |
| `docker` | Read-only commands only (useful for debugging) |
| `ur` | Read-only commands only (useful for debugging) |
| `cargo` | Blocks `install`, `uninstall`, `--manifest-path`, and other commands that escape the directory |

#### Custom Commands

Users can add additional Lua scripts for commands of their choosing and enable them on a per-project basis. While not provided by default, MCP servers that use STDIO can also be configured to run on the host.

## Communicating with the Worker

Workers run Claude Code inside of tmux. When the server needs to instruct the worker, it sends a gRPC message to that worker's **workerd**, which then uses `tmux send-keys` to send a message to the Claude session. The `Stop` Claude hook informs workerd that Claude is ready to receive instruction.

The worker can also be directly attached to from the host via docker commands, which can be invoked via `ur worker attach`.

## Security Considerations

### Accepted Risk

The fact that the system runs any code at all is a security vulnerability that has been left open for the sake of system performance (namely, the amount of RAM it requires to run a thick worker with full build dependencies), as well as some more tricky concerns (yubikey signing of git commits).

That said, Claude Code running on the host machine already has the power to write and execute arbitrary code, and a simple `go test` holds the potential for unlimited compromises. The goal of this project is to make it *safer* and *faster* to run Claude Code for Agentic Development. It is explicitly not trying to create a hermetic environment for maximum security Agentic Development. Given that the level of code exposure remains the same with Ur ADE as without it, this vulnerability has been accepted.

### Other Security Measures

* Cargo Audit checks for dependency vulnerabilities as part of the CI process
* The pre-push process automatically updates cargo dependencies
* A custom fork of the **sqlx** crate removes a vulnerable **rsa** dependency (only used by **mysql** features)
* All interprocess communication uses unauthenticated gRPC — since all network communication is entirely constrained to the host computer, this was deemed safe
* Pre-push hooks run full CI including Cargo Audit; worker nodes cannot bypass these gates unless explicitly permitted
