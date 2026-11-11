FROM debian:bookworm-slim

# Bookworm's tmux 3.3a strips OSC 8 hyperlinks. Use backports so long
# authentication links retain their complete target across display rows.
RUN printf 'deb http://deb.debian.org/debian bookworm-backports main\n' \
        > /etc/apt/sources.list.d/bookworm-backports.list && \
    apt-get update && apt-get install -y --no-install-recommends \
    bash \
    bash-completion \
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
    && apt-get install -y --no-install-recommends -t bookworm-backports tmux \
    && dpkg --compare-versions "$(dpkg-query -W -f='${Version}' tmux)" ge 3.4 \
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

# The agent-agnostic skills and instructions in this directory
# (potential-skills/, instructions/, shared-instructions/, vendored
# superpowers skills) are NOT baked here. They change often, and anything that
# changes this image invalidates every layer of the agent images built FROM it,
# including the slow agent CLI download. Each agent Dockerfile copies them in
# near the end instead, via the `shared` named build context
# (scripts/build/image.sh passes --build-context shared=containers/worker-base).
# Only the worker-owned directory is created here.
RUN mkdir -p /home/worker/.agent-shared && chown worker:worker /home/worker/.agent-shared
