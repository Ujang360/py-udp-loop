#!/usr/bin/env bash
set -ex

pip install -U maturin
maturin build --release --strip
maturin develop --release --strip
