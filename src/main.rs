// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! gg-log-manager: Greengrass log management component.

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
    fn modules_compile() {
        // Verify all modules are accessible
        let _ = super::config::module_name();
        let _ = super::credentials::module_name();
        let _ = super::disk::module_name();
        let _ = super::scanner::module_name();
        let _ = super::uploader::module_name();
    }
}
