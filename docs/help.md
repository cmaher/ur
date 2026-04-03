# Ur ADE

A control plane for

* Creating and managing tickets (units of work)
* Dispatching agents to autonomously work on tickets until completion

## Workflow

1. Create a ticket from the Tickets page ('C')
2. Enter your description into the ticket
2a. If this ticket has enough deatil for immediate dispatch, make it a "code" ticket. (Only recommended for well-defined tasks)
3. Save and close the file
4. Choose "Create and Dispatch"
5. For a design ticket, attach to the worker in another terminal tab with `ur worker attach <ticket id>`
6. The design results in a new group of tickets. Select the parent row and hit "D" to dispatch
7. Monitor the progress in the flows tab. Optionally attach to the running agent with `ur worker attach <ticket id>`
8. When the work is complete, you will get a pull request in github.
9. Comment on whatever needs changing
10. Comment "ur respond" to have the agent address the feedback immediately, or "ur approve" tohave the agent create tickets for later and merge immediately.
11. Repeat until you decide to approve

### Lifecycle

* implementing -> verifying -> pushing -> in\_review -> create\_feedback -> merging -> done
* If the worker creates feedback or encounter errors anywhere in the cycle, the worker goes back to `implementing`.
* After 6 cycles in implemetning (from errors, not feedback), the worker will stall and require manual intervention.
    * Attach to the worker with `ur worker attach <ticket-id>` and have a chat with it.
    * 'V' for 'Verifying' on the flows page will un-stall the worker

## Customization

URCONFIG (defaults to ~/.ur/ur.toml) provides a great deal of customization options

For any directories, you can use the following variables:
* "%URCONFIG%" - path relative to $URCONFIG env var
* "%PROJECT%" - path relative to the git repo

### Projects

* Ur works with projects, which are essentially a short key associated with a git repo (e.g. "%PROJECT%/ur-hooks")
* Each project has a cache of git directories stored in ~/.ur/workspace by default
* You can access the directory used by a worker with:
    * `ur worker dir <ticket-id>` -- show the directory
    * `ur worker code <ticket-id>` -- open the directory in vscode

The following is an example configuration used for this project

```
[projects.ur]
repo = "https://github.com/cmaher/ur.git"

[projects.ur.container]
# image can be any docker image on your system. derive custom images from ur-worker:latest
# the rust worker assumes that you are using `bacon`
image = "ur-worker-rust"
# mounts for useful reference. I mount logs for debugging. The :ro is for read-only.
mounts = ["%URCONFIG%/logs:/context/logs:ro"]

[projects.ur.tui]
# themes can be configured globally and per-project
theme = "aqua"
```

### Hooks

There are two primary integration points for hooks. By default, the `ur-hooks`  directory in your project will be used:

* `ur-hooks/git` -- git hooks that are copied into your cahced repos
    * The "pre-push" hook is very useful for ensuring that the host runs verifications before pushing. If this hook fails, the worker will automatically fix issues.
* `ur-hooks/skills/<skill>` -- markdown snippets that get loaded into the various skills that drive the flows. Only Implement is currently supported:
    * `implement/after-ticket-claim.md` -- After a ticket is claimed, but before work is commenced
    * `implement/before-dispatch.md` -- Before dispatching a subagent to work on a ticket
    * `implmenet/subtask-verifications.md` -- Tasks (targeted tests, lints, etc.) to run after each ticket in a dispatched group of tickets
    * `implement/final-verifications.md` -- Tasks to run after all tickets have been completed. (full ci, fmt, etc.)

Hooks directories are configurable with:
* projects.<key>.git\_hooks\_dir
* projects.<key>.skill\_hooks\_dir

### Host Commands

Allow and configure more commands to run on the host. Commands are configured globally and allow-listed per-project.

Example configuration:

```
# configure the godot-mcp command
[hostexec.commands.godot-mcp]
# bidirectional - useful for stdio mcp
bidi = true
# keep the command alive
long_lived = true

# some godot project
[projects.gd]
# enable the use of godot-mcp
hostexec = ["godot-mcp"]
```

### Database backup

I backup my database to google drive.

```
[backup]
interval\_minutes = 30
path = "/some/path/on/host"
```
