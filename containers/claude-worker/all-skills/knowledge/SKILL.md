---
name: knowledge
description: Create, update, or delete knowledge base documents with proper tags and descriptions
---

# Knowledge Base Management

Manage knowledge documents — create new entries, update existing ones, or delete outdated content. Walk the user through each step interactively.

## Determine the Operation

Ask the user what they want to do:

1. **Create** a new knowledge document
2. **Update** an existing knowledge document
3. **Delete** a knowledge document

If the user's intent is clear from `$ARGUMENTS`, skip asking and proceed directly.

---

## Create a Knowledge Document

### Step 1: Scope Selection

Ask: **"Should this be project-scoped or shared?"**

- **Project-scoped** (`-p <project>`): knowledge specific to one project
- **Shared** (`--shared`): knowledge useful across all projects

### Step 2: Title

Ask the user for a clear, descriptive title for the document.

### Step 3: Description

Ask the user for a short description (one sentence summarizing the document's purpose).

**Validate**: the description must be 120 characters or fewer. If it exceeds 120 characters, ask the user to shorten it before proceeding.

### Step 4: Body

Ask the user for the full body content. This is the main knowledge content — it can be as long as needed.

### Step 5: Tags

First, show existing tags so the user can reuse them:

```bash
ur knowledge list-tags --output json          # for shared
ur knowledge list-tags -p <project> --output json  # for project-scoped
```

Present the existing tags to the user and ask them to select from existing tags or create new ones. Multiple tags are encouraged for discoverability.

### Step 6: Create

Run the create command with all gathered information:

```bash
ur knowledge create "<title>" \
  -p <project>    # or --shared \
  -d "<description>" \
  -b "<body>" \
  -t <tag1> -t <tag2> \
  --output json
```

Confirm success and show the created document ID.

---

## Update a Knowledge Document

### Step 1: Identify the Document

Ask the user which document to update. If they don't know the ID, help them find it:

```bash
ur knowledge list --output json               # all
ur knowledge list -p <project> --output json   # by project
ur knowledge list --shared --output json       # shared only
ur knowledge list --tag <tag> --output json    # by tag
```

### Step 2: Read Current Content

Fetch the current document so the user can see what exists:

```bash
ur knowledge read <id> --output json
```

Show the current title, description, body, and tags to the user.

### Step 3: Gather Changes

Ask what they want to change (title, description, body, tags — any combination). Validate that any new description is 120 characters or fewer.

When updating tags, show existing tags for reuse:

```bash
ur knowledge list-tags --output json
ur knowledge list-tags -p <project> --output json
```

### Step 4: Apply Update

```bash
ur knowledge update <id> \
  --title "<new title>" \      # if changing
  -d "<new description>" \     # if changing
  -b "<new body>" \            # if changing
  -t <tag1> -t <tag2> \        # if changing (replaces all tags)
  --output json
```

Confirm the update succeeded.

---

## Delete a Knowledge Document

### Step 1: Identify the Document

Same as update — help the user find the document ID if needed using `ur knowledge list`.

### Step 2: Confirm

Read the document with `ur knowledge read <id> --output json` and show it to the user. Ask for explicit confirmation before deleting.

### Step 3: Delete

```bash
ur knowledge delete <id> --output json
```

Confirm deletion.

$ARGUMENTS
