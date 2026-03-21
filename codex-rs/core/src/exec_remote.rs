use std::io;
use std::time::Instant;

use codex_exec_server::Environment as ExecutorEnvironment;
use codex_exec_server::ExecOutputStream as ExecutorOutputStream;
use codex_exec_server::ExecParams as ExecutorExecParams;
use codex_exec_server::ExecProcess;
use codex_exec_server::ProcessOutputChunk as ExecutorProcessOutputChunk;
use codex_exec_server::ReadParams as ExecutorReadParams;
use uuid::Uuid;

use super::AGGREGATE_BUFFER_INITIAL_CAPACITY;
use super::CodexErr;
use super::EXIT_CODE_SIGNAL_BASE;
use super::ExecCapturePolicy;
use super::ExecExpiration;
use super::ExecRequest;
use super::ExecToolCallOutput;
use super::MAX_EXEC_OUTPUT_DELTAS_PER_CALL;
use super::READ_CHUNK_SIZE;
use super::RawExecToolCallOutput;
use super::Result;
use super::SIGKILL_CODE;
use super::StdoutStream;
use super::StreamOutput;
use super::TIMEOUT_CODE;
use super::aggregate_output;
use super::append_capped;
use super::emit_output_delta;
use super::finalize_exec_result;
use super::synthetic_exit_status;

pub(crate) async fn execute_exec_request_via_environment(
    exec_request: ExecRequest,
    environment: &ExecutorEnvironment,
    stdout_stream: Option<StdoutStream>,
    after_spawn: Option<Box<dyn FnOnce() + Send>>,
) -> Result<ExecToolCallOutput> {
    let ExecRequest {
        command,
        cwd,
        mut env,
        network,
        expiration,
        capture_policy,
        sandbox,
        windows_sandbox_level: _,
        windows_sandbox_private_desktop: _,
        sandbox_permissions: _,
        sandbox_policy: _,
        file_system_sandbox_policy: _,
        network_sandbox_policy: _,
        justification: _,
        arg0,
    } = exec_request;

    if let Some(network) = network.as_ref() {
        network.apply_to_env(&mut env);
    }

    let process_id = format!("shell-{}", Uuid::new_v4());
    let params = ExecutorExecParams {
        process_id: process_id.clone(),
        argv: command,
        cwd,
        env,
        tty: false,
        arg0,
    };

    let executor = environment.get_executor();
    let start = Instant::now();
    executor
        .start(params)
        .await
        .map_err(exec_server_error_to_codex)?;
    if let Some(after_spawn) = after_spawn {
        after_spawn();
    }

    let raw_output_result = consume_exec_server_output(
        executor,
        &process_id,
        expiration,
        capture_policy,
        stdout_stream,
    )
    .await;
    let duration = start.elapsed();
    finalize_exec_result(raw_output_result, sandbox, duration)
}

async fn consume_exec_server_output(
    executor: std::sync::Arc<dyn ExecProcess>,
    process_id: &str,
    expiration: ExecExpiration,
    capture_policy: ExecCapturePolicy,
    stdout_stream: Option<StdoutStream>,
) -> Result<RawExecToolCallOutput> {
    let retained_bytes_cap = capture_policy.retained_bytes_cap();
    let mut stdout = Vec::with_capacity(
        retained_bytes_cap.map_or(AGGREGATE_BUFFER_INITIAL_CAPACITY, |max_bytes| {
            AGGREGATE_BUFFER_INITIAL_CAPACITY.min(max_bytes)
        }),
    );
    let mut stderr = Vec::with_capacity(stdout.capacity());
    let mut after_seq = None;
    let mut exit_status = None;
    let mut timed_out = false;
    let mut emitted_deltas = 0usize;

    let expiration_wait = async {
        if capture_policy.uses_expiration() {
            expiration.wait().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(expiration_wait);

    loop {
        let read_future = executor.read(ExecutorReadParams {
            process_id: process_id.to_string(),
            after_seq,
            max_bytes: Some(READ_CHUNK_SIZE),
            wait_ms: Some(50),
        });
        tokio::pin!(read_future);

        let read_response = tokio::select! {
            response = &mut read_future => response.map_err(exec_server_error_to_codex)?,
            _ = &mut expiration_wait => {
                timed_out = true;
                let _ = executor.terminate(process_id).await;
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                let _ = executor.terminate(process_id).await;
                exit_status = Some(synthetic_exit_status(EXIT_CODE_SIGNAL_BASE + SIGKILL_CODE));
                break;
            }
        };

        after_seq = Some(read_response.next_seq.saturating_sub(1));
        append_exec_server_chunks(
            read_response.chunks,
            &mut stdout,
            &mut stderr,
            retained_bytes_cap,
            stdout_stream.as_ref(),
            &mut emitted_deltas,
        )
        .await;

        if read_response.exited {
            exit_status = Some(synthetic_exit_status(read_response.exit_code.unwrap_or(-1)));
            loop {
                let drain_response = executor
                    .read(ExecutorReadParams {
                        process_id: process_id.to_string(),
                        after_seq,
                        max_bytes: Some(READ_CHUNK_SIZE),
                        wait_ms: Some(0),
                    })
                    .await
                    .map_err(exec_server_error_to_codex)?;
                if drain_response.chunks.is_empty() {
                    break;
                }
                after_seq = Some(drain_response.next_seq.saturating_sub(1));
                append_exec_server_chunks(
                    drain_response.chunks,
                    &mut stdout,
                    &mut stderr,
                    retained_bytes_cap,
                    stdout_stream.as_ref(),
                    &mut emitted_deltas,
                )
                .await;
            }
            break;
        }
    }

    let stdout = StreamOutput {
        text: stdout,
        truncated_after_lines: None,
    };
    let stderr = StreamOutput {
        text: stderr,
        truncated_after_lines: None,
    };
    let aggregated_output = aggregate_output(&stdout, &stderr, retained_bytes_cap);

    Ok(RawExecToolCallOutput {
        exit_status: exit_status
            .unwrap_or_else(|| synthetic_exit_status(EXIT_CODE_SIGNAL_BASE + TIMEOUT_CODE)),
        stdout,
        stderr,
        aggregated_output,
        timed_out,
    })
}

async fn append_exec_server_chunks(
    chunks: Vec<ExecutorProcessOutputChunk>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
    retained_bytes_cap: Option<usize>,
    stdout_stream: Option<&StdoutStream>,
    emitted_deltas: &mut usize,
) {
    for chunk in chunks {
        let bytes = chunk.chunk.into_inner();
        let is_stderr = chunk.stream == ExecutorOutputStream::Stderr;
        if *emitted_deltas < MAX_EXEC_OUTPUT_DELTAS_PER_CALL {
            emit_output_delta(stdout_stream, is_stderr, bytes.clone()).await;
            *emitted_deltas += 1;
        }

        match chunk.stream {
            ExecutorOutputStream::Stderr => append_with_cap(stderr, &bytes, retained_bytes_cap),
            ExecutorOutputStream::Stdout | ExecutorOutputStream::Pty => {
                append_with_cap(stdout, &bytes, retained_bytes_cap)
            }
        }
    }
}

fn append_with_cap(dst: &mut Vec<u8>, src: &[u8], max_bytes: Option<usize>) {
    if let Some(max_bytes) = max_bytes {
        append_capped(dst, src, max_bytes);
    } else {
        dst.extend_from_slice(src);
    }
}

fn exec_server_error_to_codex(err: codex_exec_server::ExecServerError) -> CodexErr {
    CodexErr::Io(io::Error::other(err.to_string()))
}
