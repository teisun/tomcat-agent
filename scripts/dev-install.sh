#!/bin/sh
# Only Cargo owns building and locating the executable. Never bypass Agent protection.
cd "$(dirname "$0")/../tomcat" || exit 1
exec cargo run --release --bin tomcat -- self-install
