#!/bin/bash
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PYTHONPATH="${DIR}/frontend/src"
export PROXY_GENERATOR_BACKEND="${DIR}/backend/target/release/proxy-generator-backend"
exec python -m proxy_generator
