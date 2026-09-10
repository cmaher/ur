FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    bash-completion \
    tmux \
    curl \
    ca-certificates \
    coreutils \
    jq \
    yq \
    less \
    locales \
    make \
    perl \
    python3 \
    ripgrep \
    && sed -i 's/^# *en_US.UTF-8/en_US.UTF-8/' /etc/locale.gen \
    && locale-gen \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8
ENV LC_ALL=en_US.UTF-8

COPY .tmux.conf /etc/tmux.conf

# Create a non-root user (Claude Code refuses --dangerously-skip-permissions as root)
RUN useradd -m -s /bin/bash worker

# Bake in a clean shell profile. Debian's default ~/.profile modifies PATH
# in ways that can conflict with our ENV PATH ordering in the Dockerfile.
COPY worker.profile /home/worker/.profile
COPY worker.bashrc /home/worker/.bashrc
RUN chown worker:worker /home/worker/.profile /home/worker/.bashrc

# Agent-agnostic skills and instructions, shared by every agent image built
# on top of this one (see containers/worker-claude/). Agent-specific assets
# (CLI install, app config) live in the agent layer instead.
# Order matters: both COPYs land in the same directory, and the later one wins
# a name collision. The repo's own skills go second so they override a
# same-named vendored skill, not the other way round.
COPY vendor/superpowers/skills/ /home/worker/.agent-shared/potential-skills/
COPY potential-skills/ /home/worker/.agent-shared/potential-skills/
COPY instructions/ /home/worker/.agent-shared/instructions/
COPY shared-instructions/ /home/worker/.agent-shared/shared-instructions/
RUN chown -R worker:worker /home/worker/.agent-shared
