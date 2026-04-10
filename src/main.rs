// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! gg-log-manager: Greengrass LogManager Generic Type Component.

mod config;
mod credentials;
mod disk;
mod scanner;
mod uploader;

fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!("gg-log-manager starting");
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_modules_compile() {
        assert!(true);
    }
}
