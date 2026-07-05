#!/usr/bin/env bash
# Conformance oracle: proves nbspec's requirement/scenario/delta grammar
# remains syntactically compatible with upstream OpenSpec tooling.
#
# Renders the grammar fixtures (tests/fixtures/conformance) into the
# upstream spec-driven layout inside a scratch workspace and runs a
# PINNED upstream validator against them. Layout conformance is not
# claimed - nbspec's default schema deliberately diverges from the
# spec-driven layout; only the requirement/scenario/delta grammar is
# under test.
#
# Development-time only and informational: nbspec never depends on the
# openspec binary at runtime, and this oracle retires if nbspec's
# grammar deliberately diverges. Override the pin with
# OPENSPEC_ORACLE_VERSION for exploratory runs; bump the default only
# deliberately, alongside a grammar review.
set -euo pipefail

version_pin="${OPENSPEC_ORACLE_VERSION:-1.5.0}"
repository_root="$(git rev-parse --show-toplevel)"
fixtures="${repository_root}/tests/fixtures/conformance"
workspace="${repository_root}/.auxiliary/temporary/conformance-oracle"
change_id="conformance-fixtures"
change_directory="${workspace}/openspec/changes/${change_id}"

rm -rf "${workspace}"
mkdir -p "${workspace}/openspec/specs" "${change_directory}/specs"
printf 'schema: spec-driven\n' > "${workspace}/openspec/config.yaml"

cat > "${change_directory}/proposal.md" <<'EOF'
## Why

Continuously prove that documents written in nbspec's grammar are
accepted by the pinned upstream OpenSpec validator.

## What Changes

- Exercise ADDED, MODIFIED, REMOVED, and RENAMED delta sections
  against upstream validation.
EOF

cat > "${change_directory}/tasks.md" <<'EOF'
## 1. Conformance

- [ ] 1.1 Validate the grammar fixtures with the upstream validator
EOF

for fixture in "${fixtures}"/deltas/*.md; do
    capability="$(basename "${fixture}" .md)"
    mkdir -p "${change_directory}/specs/${capability}"
    cp "${fixture}" "${change_directory}/specs/${capability}/spec.md"
done

for fixture in "${fixtures}"/specs/*.md; do
    capability="$(basename "${fixture}" .md)"
    mkdir -p "${workspace}/openspec/specs/${capability}"
    cp "${fixture}" "${workspace}/openspec/specs/${capability}/spec.md"
done

if command -v openspec > /dev/null \
    && [ "$(openspec --version)" = "${version_pin}" ]
then validator=( openspec )
else validator=( npx --yes "@fission-ai/openspec@${version_pin}" )
fi

cd "${workspace}"
"${validator[@]}" validate --all --strict
printf '\nConformance oracle passed against openspec %s.\n' "${version_pin}"
