CARGO := @cargo

NEXTEST_RUN_ARGS := --no-fail-fast --success-output immediate --failure-output final

#
# Check
#

check:
	${CARGO} check --workspace

fmt:
	${CARGO} fmt --all --check

clippy:
	${CARGO} clippy --workspace --tests -- --deny warnings

test:
	${CARGO} nextest run ${NEXTEST_RUN_ARGS} --workspace types::extension::packed::test_service

#
# Build
#

doc:
	${CARGO} doc --workspace --no-deps

build:
	${CARGO} build --workspace

release:
	${CARGO} build --workspace --release
