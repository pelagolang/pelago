# A fixed toolchain for publishable benchmark runs (versions: bench/toolchains.toml).
#   docker build -t pelago-bench -f bench/Dockerfile .
#   docker run --rm --cpuset-cpus=2 -v "$PWD:/w" -w /w pelago-bench python3 bench/harness/run.py --pin 0 --size small,medium,large --save
# The harness writes every version it finds into the results and says when one differs from toolchains.toml.
# Built and checked on 2026-10-23: every tool reports the version pinned in toolchains.toml.
FROM gcc:16.2

RUN apt-get update && apt-get install -y --no-install-recommends python3 curl ca-certificates git make lsb-release wget gnupg \
    && rm -rf /var/lib/apt/lists/*

# Clang 23 from apt.llvm.org
RUN curl -fsSL https://apt.llvm.org/llvm.sh -o /tmp/llvm.sh && bash /tmp/llvm.sh 23 \
    && ln -sf /usr/bin/clang-23 /usr/local/bin/clang && ln -sf /usr/bin/clang++-23 /usr/local/bin/clang++ && rm /tmp/llvm.sh

# TinyCC from a pinned commit of the mob branch
ARG TCC_COMMIT=e23dabed154a8495f3ac1ff3cc9d04fa1c207893
RUN git clone https://repo.or.cz/tinycc.git /tmp/tinycc && cd /tmp/tinycc && git checkout "$TCC_COMMIT" \
    && ./configure && make -j"$(nproc)" && make install && rm -rf /tmp/tinycc

# Rust, exact version
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.99.1
