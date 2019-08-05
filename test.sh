#!/usr/bin/env bash

cargo build && cp -f ./target/debug/libatsdump.so atsdump.pd_linux && pd test.pd
