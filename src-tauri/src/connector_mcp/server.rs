//! OS-protected local transport; no TCP listener, no remotely reachable IPC.
use super::{ipc, BridgeResult, IpcRequest, PairCredential, DEADLINE};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

pub(super) struct LocalServer {
    pub endpoint: String,
    task: JoinHandle<()>,
    #[cfg(unix)]
    directory: std::path::PathBuf,
}

impl LocalServer {
    pub fn running(&self) -> bool {
        !self.task.is_finished()
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.task.abort();
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.endpoint);
            let _ = std::fs::remove_dir(&self.directory);
        }
    }
}

async fn serve_one<S: AsyncRead + AsyncWrite + Unpin>(app: AppHandle, mut stream: S) {
    let operation = async {
        let request: IpcRequest = ipc::read_frame(&mut stream, ipc::MAX_REQUEST).await?;
        let result = super::dispatch(&app, request).await;
        ipc::write_frame(&mut stream, &result, ipc::MAX_RESPONSE).await
    };
    // Errors intentionally contain no remote body and are not logged.
    let _: Result<BridgeResult<()>, _> = tokio::time::timeout(DEADLINE, operation).await;
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn session_for(pid: u32) -> BridgeResult<u32> {
        let mut session = 0;
        // SAFETY: session is a valid output pointer; no handles are retained.
        unsafe { ProcessIdToSessionId(pid, &mut session) }
            .map_err(|_| "ipc_identity_unavailable")?;
        Ok(session)
    }

    fn current_user_sid() -> BridgeResult<String> {
        let mut token = HANDLE::default();
        // SAFETY: query-only current process token; all allocated resources are
        // freed on every return path below, and TOKEN_USER is read from aligned
        // backing storage after the OS confirmed its length.
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(|_| "ipc_identity_unavailable")?;
            let result = (|| {
                let mut needed = 0;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
                if needed == 0 || needed > 64 * 1024 {
                    return Err("ipc_identity_unavailable".into());
                }
                let mut storage =
                    vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
                GetTokenInformation(
                    token,
                    TokenUser,
                    Some(storage.as_mut_ptr().cast()),
                    needed,
                    &mut needed,
                )
                .map_err(|_| "ipc_identity_unavailable")?;
                let user = &*storage.as_ptr().cast::<TOKEN_USER>();
                let mut sid = PWSTR::null();
                ConvertSidToStringSidW(user.User.Sid, &mut sid)
                    .map_err(|_| "ipc_identity_unavailable")?;
                let result = sid
                    .to_string()
                    .map_err(|_| "ipc_identity_unavailable".to_owned());
                let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
                result
            })();
            let _ = CloseHandle(token);
            result
        }
    }

    fn create_pipe(endpoint: &str, first: bool) -> BridgeResult<NamedPipeServer> {
        let sid = current_user_sid()?;
        // Protected DACL: deny network logons, allow only current user/SYSTEM.
        // `reject_remote_clients` separately prevents remote named-pipe access.
        let sddl: Vec<u16> = format!("D:P(D;;GA;;;NU)(A;;GA;;;SY)(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: SDDL remains alive through conversion, descriptor through pipe
        // creation. CreateNamedPipe copies it; LocalFree then releases the OS allocation.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
            .map_err(|_| "ipc_security_unavailable")?;
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let result = ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .max_instances(16)
                .create_with_security_attributes_raw(
                    endpoint,
                    (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                );
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            result.map_err(|_| "ipc_unavailable".into())
        }
    }

    fn client_in_current_session(pipe: &NamedPipeServer) -> bool {
        let mut pid = 0;
        // SAFETY: pipe owns a valid named-pipe handle for the duration of call.
        if unsafe { GetNamedPipeClientProcessId(HANDLE(pipe.as_raw_handle()), &mut pid) }.is_err() {
            return false;
        }
        matches!((session_for(pid), session_for(std::process::id())), (Ok(a), Ok(b)) if a == b)
    }

    pub async fn start(app: AppHandle) -> BridgeResult<LocalServer> {
        let endpoint = format!(r"\\.\pipe\Transcreve-{}", uuid::Uuid::new_v4());
        let mut listener = create_pipe(&endpoint, true)?;
        let name = endpoint.clone();
        let permits = Arc::new(Semaphore::new(8));
        let task = tokio::spawn(async move {
            loop {
                if listener.connect().await.is_err() {
                    break;
                }
                // Keep at least one server handle open, preventing pipe squatting.
                let next = match create_pipe(&name, false) {
                    Ok(next) => next,
                    Err(_) => break,
                };
                let connected = std::mem::replace(&mut listener, next);
                if !client_in_current_session(&connected) {
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let app = app.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    serve_one(app, connected).await;
                });
            }
        });
        Ok(LocalServer { endpoint, task })
    }

    pub async fn connect(credential: &PairCredential) -> BridgeResult<NamedPipeClient> {
        if !credential.endpoint.starts_with(r"\\.\pipe\Transcreve-")
            || credential.endpoint.len() > 100
        {
            return Err("invalid_endpoint".into());
        }
        let client = ClientOptions::new()
            .open(&credential.endpoint)
            .map_err(|_| "app_unavailable")?;
        let mut pid = 0;
        // Authenticate the expected app process before sending the local secret.
        // SAFETY: client owns a valid pipe handle and pid is valid output storage.
        unsafe { GetNamedPipeServerProcessId(HANDLE(client.as_raw_handle()), &mut pid) }
            .map_err(|_| "ipc_identity_unavailable")?;
        if pid != credential.server_pid || session_for(pid)? != session_for(std::process::id())? {
            return Err("app_unavailable".into());
        }
        Ok(client)
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use tokio::net::{UnixListener, UnixStream};

    pub async fn start(app: AppHandle) -> BridgeResult<LocalServer> {
        let directory =
            std::env::temp_dir().join(format!("transcreve-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|_| "ipc_security_unavailable")?;
        let path = directory.join("bridge.sock");
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(_) => {
                let _ = std::fs::remove_dir(&directory);
                return Err("ipc_unavailable".into());
            }
        };
        if std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).is_err() {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(&directory);
            return Err("ipc_security_unavailable".into());
        }
        let uid = std::fs::metadata(&directory)
            .map_err(|_| "ipc_security_unavailable")?
            .uid();
        let endpoint = path.to_str().ok_or("ipc_unavailable")?.to_owned();
        let task = tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(8));
            while let Ok((stream, _)) = listener.accept().await {
                if !stream.peer_cred().is_ok_and(|peer| peer.uid() == uid) {
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    continue;
                };
                let app = app.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    serve_one(app, stream).await;
                });
            }
        });
        Ok(LocalServer {
            endpoint,
            task,
            directory,
        })
    }

    pub async fn connect(credential: &PairCredential) -> BridgeResult<UnixStream> {
        let path = std::path::Path::new(&credential.endpoint);
        let parent = path.parent().ok_or("invalid_endpoint")?;
        if path.file_name().and_then(|v| v.to_str()) != Some("bridge.sock")
            || !parent
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.starts_with("transcreve-mcp-"))
        {
            return Err("invalid_endpoint".into());
        }
        let directory = std::fs::symlink_metadata(parent).map_err(|_| "app_unavailable")?;
        let socket = std::fs::symlink_metadata(path).map_err(|_| "app_unavailable")?;
        if !directory.is_dir()
            || directory.permissions().mode() & 0o777 != 0o700
            || socket.permissions().mode() & 0o777 != 0o600
            || directory.uid() != socket.uid()
        {
            return Err("ipc_security_unavailable".into());
        }
        let stream = UnixStream::connect(path)
            .await
            .map_err(|_| "app_unavailable")?;
        let peer = stream.peer_cred().map_err(|_| "ipc_identity_unavailable")?;
        if peer.uid() != directory.uid()
            || peer
                .pid()
                .is_some_and(|pid| pid as u32 != credential.server_pid)
        {
            return Err("ipc_identity_unavailable".into());
        }
        Ok(stream)
    }
}

pub(super) use platform::{connect, start};
