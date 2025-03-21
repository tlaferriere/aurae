/* -------------------------------------------------------------------------- *\
 *        Apache 2.0 License Copyright © 2022-2023 The Aurae Authors          *
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

use super::isolation_controls::{Isolation, IsolationControls};
use crate::AURAE_RUNTIME_DIR;
use client::AuraeConfig;
use clone3::Flags;
use nix::{
    libc::SIGCHLD,
    sys::signal::{Signal, SIGKILL, SIGTERM},
    unistd::Pid,
};
use std::{
    io::{self, ErrorKind},
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Command, ExitStatus},
};
use tracing::{error, info, trace};

#[derive(Debug)]
pub struct NestedAuraed {
    process: procfs::process::Process,
    #[allow(unused)]
    pidfd: i32,
    #[allow(unused)]
    iso_ctl: IsolationControls,
    pub client_config: AuraeConfig,
}

impl NestedAuraed {
    pub fn new(name: String, iso_ctl: IsolationControls) -> io::Result<Self> {
        // Here we launch a nested auraed with the --nested flag
        // which is used our way of "hooking" into the newly created
        // aurae isolation zone.

        let random = uuid::Uuid::new_v4();

        // TODO: handle expect
        let mut client_config =
            AuraeConfig::try_default().expect("file based config");
        client_config.system.socket =
            format!("{AURAE_RUNTIME_DIR}/aurae-{random}.sock");

        let mut command = Command::new("auraed");
        let _ = command.current_dir("/").args([
            "--socket",
            &client_config.system.socket,
            "--nested", // NOTE: for now, the nested flag only signals for the code in the init module to not trigger (i.e., don't run the pid 1 code, run the non pid 1 code)
        ]);

        // We have a concern that the "command" API make change/break in the future and this
        // test is intended to help safeguard against that!
        // We check that the command we kept has the expected number of args following the call
        // to command.args, whose return value we ignored above.
        assert_eq!(command.get_args().len(), 3);

        // *****************************************************************
        // ██████╗██╗      ██████╗ ███╗   ██╗███████╗██████╗
        // ██╔════╝██║     ██╔═══██╗████╗  ██║██╔════╝╚════██╗
        // ██║     ██║     ██║   ██║██╔██╗ ██║█████╗   █████╔╝
        // ██║     ██║     ██║   ██║██║╚██╗██║██╔══╝   ╚═══██╗
        // ╚██████╗███████╗╚██████╔╝██║ ╚████║███████╗██████╔╝
        // ╚═════╝╚══════╝ ╚═════╝ ╚═╝  ╚═══╝╚══════╝╚═════╝
        // Clone docs: https://man7.org/linux/man-pages/man2/clone.2.html
        // *****************************************************************

        // Prepare clone3 command to "execute" the nested auraed
        let mut clone = clone3::Clone3::default();

        // [ Options ]

        // If the child fails to start, indicate an error
        // Set the pid file descriptor to -1
        let mut pidfd = -1;
        let _ = clone.flag_pidfd(&mut pidfd);

        // We have a concern that the "clone" API changes/breaks in the future and this
        // test is intended to help safeguard against that!
        // We check that the clone we kept has set the first flag we set above.
        assert_eq!(clone.as_clone_args().flags, Flags::PIDFD.bits());

        // Freeze the parent until the child calls execvp
        let _ = clone.flag_vfork();

        // Manage SIGCHLD for the nested process
        // Define SIGCHLD for signal handler
        let _ = clone.exit_signal(SIGCHLD as u64);

        // [ Namespaces and Isolation ]

        let mut isolation = Isolation::new(name);
        isolation.setup(&iso_ctl)?;

        // Always unshare the Cgroup namespace
        let _ = clone.flag_newcgroup();

        // Isolate Network
        if iso_ctl.isolate_network {
            let _ = clone.flag_newnet();
        }

        // Isolate Process
        if iso_ctl.isolate_process {
            let _ = clone.flag_newpid();
            let _ = clone.flag_newns();
            let _ = clone.flag_newipc();
            let _ = clone.flag_newuts();
        }

        // Execute the clone system call and create the new process with the relevant namespaces.
        match unsafe { clone.call() }
            .map_err(|e| io::Error::from_raw_os_error(e.0))?
        {
            0 => {
                // child
                let command = {
                    unsafe {
                        command.pre_exec(move || {
                            isolation.isolate_process(&iso_ctl)?;
                            isolation.isolate_network(&iso_ctl)?;
                            Ok(())
                        })
                    }
                };

                let e = command.exec();
                error!("Unexpected exit from child command: {e:#?}");
                Err(e)
            }
            pid => {
                // parent
                info!("Nested auraed running with host pid {}", pid.clone());
                let process = procfs::process::Process::new(pid)
                    .map_err(|e| io::Error::new(ErrorKind::Other, e))?;

                Ok(Self { process, pidfd, iso_ctl, client_config })
            }
        }
    }

    /// Sends a graceful shutdown signal to the nested process.
    pub fn shutdown(&mut self) -> io::Result<ExitStatus> {
        // TODO: Here, SIGTERM works when using auraescript, but hangs(?) during unit tests.
        //       SIGKILL, however, works. The hang is avoided if the process is not isolated.
        //       Tests have not been done to figure out which namespace is the cause of the hang.
        self.do_kill(Some(SIGTERM))?;
        self.wait()
    }

    /// Sends a [SIGKILL] signal to the nested process.
    pub fn kill(&mut self) -> io::Result<ExitStatus> {
        self.do_kill(Some(SIGKILL))?;
        self.wait()
    }

    fn do_kill<T: Into<Option<Signal>>>(
        &mut self,
        signal: T,
    ) -> io::Result<()> {
        let signal = signal.into();
        let pid = Pid::from_raw(self.process.pid);

        nix::sys::signal::kill(pid, signal)
            .map_err(|e| io::Error::from_raw_os_error(e as i32))
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        let pid = Pid::from_raw(self.process.pid);

        let mut exit_status = 0;
        let _child_pid = loop {
            let res =
                unsafe { libc::waitpid(pid.as_raw(), &mut exit_status, 0) };

            if res == -1 {
                let err = io::Error::last_os_error();
                match err.kind() {
                    ErrorKind::Interrupted => continue,
                    _ => break Err(err),
                }
            }

            break Ok(res);
        }?;

        let exit_status = ExitStatus::from_raw(exit_status);

        trace!("Pid {pid} exited with status {exit_status}");

        Ok(exit_status)
    }

    pub fn pid(&self) -> Pid {
        Pid::from_raw(self.process.pid)
    }
}
