// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! gg-log-manager: Greengrass LogManager Generic Type Component.
//!
//! Rust implementation of aws.greengrass.LogManager for GG Classic and GG Lite.
//! Tails log files and EMF JSON files, uploads to CloudWatch Logs.

mod config;
mod credentials;
mod disk;
mod scanner;
mod uploader;

use tracing::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    info!("gg-log-manager starting");
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_modules_compile() {
        assert!(true);
    }
}
