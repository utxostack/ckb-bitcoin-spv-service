CARGO := @cargo

#
# Check
#

check:
	${CARGO} check --workspace

fmt:
	${CARGO} fmt --all --check

clippy:
	${CARGO} clippy --workspace --tests -- --deny warnings

#
# Build
#

doc:
	${CARGO} doc --workspace --no-deps

build:
	${CARGO} build --workspace

release:
	${CARGO} build --workspace --release
