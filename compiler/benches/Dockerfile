# A fixed toolchain for publishable benchmark runs: docker build -t pelago-bench -f compiler/benches/Dockerfile .
# then: docker run --rm --cpuset-cpus=2 -v "$PWD:/w" -w /w pelago-bench python3 compiler/benches/harness.py --pin 0 --md bench/results.md
# The Ubuntu tag fixes gcc, g++, clang and tcc to that release's packages; the harness writes every version it finds into the results.
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends gcc g++ clang tcc python3 curl ca-certificates git build-essential \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.97.0
