# **Ur: Feature Specification & Product Requirements**

## **1\. Product Vision: Ticket-Driven Orchestration**

Ur is a coordination framework for autonomous coding agents (Claude Code), but at its heart, it is a **ticket-driven orchestration engine**.  
Agents do not just run arbitrary commands in a vacuum; every action, session, and state change is strictly bound to a Ticket. The ticket serves as the agent's prompt, its context boundary, and its ledger of work. If a task isn't tracked in a ticket, it doesn't exist to the Ur system.

## **2\. The Core Ticket System**

Ur's native ticket system is heavily inspired by markdown-based trackers (like wedow/ticket). However, instead of living as physical files alongside the repository code, **tickets are stored natively within Ur's CozoDB database** while retaining their pure, human-readable Markdown formatting.

One thing to keep in mind here is that we will be managing all tickets from whatever directory we want, so we will need some central ticket configuration (mapping tickets to porjects -- including custom prefixes)

### **2.1. Markdown & "Bead-like" Properties**

Tickets in Ur share conceptual DNA with "beads" (immutable units of context and history):

* **The Specification:** The body of the ticket contains the markdown-formatted requirements, acceptance criteria, and user intent.  
* **The Ledger:** As an agent works on a ticket, it appends "Notes" or "Comments" to the ticket. This creates an auditable trail of decisions, tool outputs, and milestone completions directly on the ticket.  
* **Context Packaging:** When an agent is assigned a ticket, the Ur Server automatically injects the ticket's markdown content into the agent's initial context window.

### **2.2. Ticket ID Schema & Nesting**

To optimize for CLI ergonomics and fast developer typing, Ur rejects the traditional hyphenated ticket IDs (e.g., PROJ-123) in favor of a **monotonic, dot-delimited schema** without hyphens.

* **Format:** \<project\_prefix\>\<ticket\_number\>.\<sub\_ticket\_number\>.\<sub\_sub\_ticket\_number\>  
* **Examples:** ic100 (Epic), ic100.1 (Task), ic100.1.1 (Sub-task)  
* **Arbitrary Nesting:** The depth is strictly arbitrary and reflects the dynamic branching of work. This provides immediate visual context of where a ticket sits in the project tree without requiring a database lookup.

### **2.3. DAGs and Dependencies**

Work is rarely flat. Tickets in Ur form a Directed Acyclic Graph (DAG), seamlessly represented by the ID schema.

* **Epic \-\> Ticket Hierarchy:** The primary relationship is parent-child, encoded directly in the ticket ID. An Epic (ic100) represents a large feature, broken down into implementable Tickets (ic100.1, ic100.2).  
* **Blocking Dependencies:** Tickets can block other tickets. The Ur Orchestrator uses this DAG to schedule agents. A Coder agent will not be automatically spawned for a ticket if its prerequisite tickets are not marked closed.  
* **Agent-Spawned Sub-tickets:** If an agent (especially one in a "Designer" role) is analyzing an Epic and realizes it requires database changes, API changes, and frontend changes, it can dynamically use agent\_tools to *spawn new sub-tickets* into the DAG (e.g., spawning ic100.1.1 from ic100.1).

## **3\. External Tracking & Egress (Jira)**

Ur acts as the local, high-speed source of truth for the agent swarm. The internal CozoDB ticket graph is what strictly drives agent behavior. While Ur can bridge to enterprise environments like Jira, this is an **optional, push-based (egress)** feature and not required for core operations.

### **3.1. No Active Polling**

* Ur **does not** actively poll Jira or listen for webhooks.  
* External systems are not allowed to arbitrarily interrupt or overwrite the local agent workflows. If a ticket needs to be worked on by the swarm, it must be explicitly created or imported into Ur.

### **3.2. Egress-Only Sync (Future/Optional)**

For environments that require visibility into agent actions, Ur can push updates out to an external tracker:

* **Status Updates:** When an agent finishes a task and uses agent\_tools done, the Ur server can automatically transition the linked Jira ticket.  
* **Note Syncing:** Important agent discoveries or summaries appended to the local ticket can be pushed to Jira as comments, keeping human stakeholders in the loop.  
* **Ticket Mirroring:** Sub-tasks spawned locally by agents can optionally be pushed up to Jira to maintain parity.

## **4\. Secure Execution & Git Proxying**

Because LLM agents are inherently unpredictable, they are executed inside fully isolated macOS containers. These containers are **air-gapped from host credentials**. There are no SSH keys, GitHub tokens, or Jira API keys mounted into the worker environment.

### **4.1. The UDS Bridge**

Communication between the isolated worker container and the trusted host monolith (the Ur Server) happens exclusively over a tightly restricted Unix Domain Socket (UDS) mounted into the container.

### **4.2. The Git Proxy**

To allow agents to collaborate on codebases without credentials, Ur uses a Git proxying architecture:

* When an agent needs to commit or push code, it does not use the standard git binary.  
* Instead, it invokes agent\_tools git \<args\>.  
* The agent\_tools CLI packages the command and current working directory, sending an RPC request over the UDS to the host server.  
* The **Ur Server** receives the request, safely translates the container path to the physical host repository path, and executes the Git command natively on the host using the developer's secure local credentials.  
* Standard output and standard error are streamed back through the socket to the agent.

This guarantees total credential security; even a fully compromised or hallucinating agent cannot leak keys or access external systems beyond what the RPC explicitly allows.

## **5\. Agent Workflows & Tools**

Agents interact with the ticket system, the host, and the user via the agent\_tools CLI inside their isolated containers.

* agent\_tools ticket read: Dumps the current ticket's markdown spec.  
* agent\_tools ticket note "Message": Appends a thought or discovery to the internal ticket ledger.  
* agent\_tools ticket spawn \--title "Add index" \--blocks CURRENT: Creates a new child ticket in the internal DAG (e.g., generates ic100.1.1 from ic100.1).  
* agent\_tools ticket status \<STATUS\>: Updates the state of the ticket (e.g., in\_progress, review, done).  
* agent\_tools git \<args\>: Proxies secure git commands to the host environment.  
* agent\_tools ask "Question": Blocks the agent's execution and sends an RPC to the host TUI, waiting for a human operator to respond to a blocking question.

## **6\. User Interface (TUI & CLI) Features**

The human operator manages the system through the Native Monolith's interfaces.

### **6.1. The TUI Dashboard**

* **The DAG Viewer:** A visual representation of the active Epic and the status of all child tickets.  
* **Fleet Status:** Shows which agents are currently assigned to which tickets, and their live execution status.  
* **The Inbox:** A queue of tickets that are Blocked\_On\_User. If an agent uses agent\_tools ask, it pauses the ticket. The user reads the prompt and answers via the TUI, unblocking the agent.

### **6.2. CLI Ticket Operations**

Users can quickly manage the queue without leaving the terminal:

* ur ticket ls: Shows the active DAG/list of open tickets.  
* ur ticket create "Fix auth bug" \--parent ic100: Creates a local ticket within CozoDB and dynamically computes the next available nested ID (e.g., ic100.2).  
* ur assign ic100.2 \--role coder: Manually forces the orchestrator to spin up an agent for a specific ticket.  
* ur process attach \<PROCESS\_ID\>: Attaches standard I/O directly to the container's underlying tmux session for manual visual debugging of the Claude Code terminal.