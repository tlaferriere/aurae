/* -------------------------------------------------------------------------- *\
 *             Apache 2.0 License Copyright © 2022 The Aurae Authors          *
 *                                                                            *
 *                +--------------------------------------------+              *
 *                |   █████╗ ██╗   ██╗██████╗  █████╗ ███████╗ |              *
 *                |  ██╔══██╗██║   ██║██╔══██╗██╔══██╗██╔════╝ |              *
 *                |  ███████║██║   ██║██████╔╝███████║█████╗   |              *
 *                |  ██╔══██║██║   ██║██╔══██╗██╔══██║██╔══╝   |              *
 *                |  ██║  ██║╚██████╔╝██║  ██║██║  ██║███████╗ |              *
 *                |  ╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═╝╚══════╝ |              *
 *                +--------------------------------------------+              *
 *                                                                            *
 *                         Distributed Systems Runtime                        *
 *                                                                            *
 * -------------------------------------------------------------------------- *
 *                                                                            *
 *   Licensed under the Apache License, Version 2.0 (the "License");          *
 *   you may not use this file except in compliance with the License.         *
 *   You may obtain a copy of the License at                                  *
 *                                                                            *
 *       http://www.apache.org/licenses/LICENSE-2.0                           *
 *                                                                            *
 *   Unless required by applicable law or agreed to in writing, software      *
 *   distributed under the License is distributed on an "AS IS" BASIS,        *
 *   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. *
 *   See the License for the specific language governing permissions and      *
 *   limitations under the License.                                           *
 *                                                                            *
\* -------------------------------------------------------------------------- */

use super::SystemRuntime;
use crate::init::{
    fs, logging, network, power::spawn_thread_power_button_listener, InitError,
    BANNER,
};
use std::{ffi::CString, path::Path};
use tonic::async_trait;
use tracing::{error, info, trace};

const POWER_BUTTON_DEVICE: &str = "/dev/input/event0";

pub(crate) struct Pid1SystemRuntime;

impl Pid1SystemRuntime {
    fn spawn_system_runtime_threads(&self) {
        // ---- MAIN DAEMON THREAD POOL ----
        // TODO: https://github.com/aurae-runtime/auraed/issues/33
        match spawn_thread_power_button_listener(Path::new(POWER_BUTTON_DEVICE))
        {
            Ok(_) => {
                info!("Spawned power button device listener");
            }
            Err(e) => {
                error!(
                    "Failed to spawn power button device listener. Error={}",
                    e
                );
            }
        }

        // ---- MAIN DAEMON THREAD POOL ----
    }
}

#[async_trait]
impl SystemRuntime for Pid1SystemRuntime {
    // Executing as PID 1 ccontext
    async fn init(self, verbose: bool) -> Result<(), InitError> {
        // Load the PID 1 execution banner
        println!("{}", BANNER);

        // Initialize the PID 1 logger
        logging::init(verbose)?;
        trace!("Logging started");

        trace!("Configure filesystem");

        // NOTE: THESE TODOS WERE ALL HERE, BUT...
        //       if you are here, you are auraed is true pid 1
        //       Container -> use container_system_runtime.rs
        //       Cell -> use cell_system_runtime.rs

        // TODO We need to determine how we want to handle mountings these filesystems.
        // TODO From within the context of a container (cgroup trailing / in cgroup namespace)
        // TODO We likely to do not need to mount these filesystems.
        // TODO Do we want to have a way to "try" these mounts and continue without erroring?

        fs::mount_vfs(
            &CString::new("none").expect("valid CString"),
            &CString::new("/dev").expect("valid CString"),
            &CString::new("devtmpfs").expect("valid CString"),
        )?;
        fs::mount_vfs(
            &CString::new("none").expect("valid CString"),
            &CString::new("/sys").expect("valid CString"),
            &CString::new("sysfs").expect("valid CString"),
        )?;
        fs::mount_vfs(
            &CString::new("proc").expect("valid CString"),
            &CString::new("/proc").expect("valid CString"),
            &CString::new("proc").expect("valid CString"),
        )?;

        trace!("configure network");
        //show_dir("/sys/class/net/", false); // Show available network interfaces
        let network = network::Network::connect()?;
        network.init().await?;
        network.show_network_info().await;

        self.spawn_system_runtime_threads();

        trace!("init of auraed as pid1 done");
        Ok(())
    }
}
