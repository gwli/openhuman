import { callCoreRpc } from '../../services/coreRpcClient';
import { isTauri } from './common';

export type TerminalKind = 'local' | 'ssh';
export type TerminalStatus = 'starting' | 'running' | 'exited' | 'closed' | 'error';

export interface TerminalSshParams {
  host: string;
  user?: string | null;
  port?: number | null;
  request_tty?: boolean;
  extra_args?: string[];
}

export interface TerminalStartParams {
  kind?: TerminalKind;
  command?: string | null;
  shell?: string | null;
  cwd?: string | null;
  rows?: number;
  cols?: number;
  ssh?: TerminalSshParams | null;
  approved?: boolean;
  actor?: string;
}

export interface TerminalSession {
  session_id: string;
  kind: TerminalKind;
  status: TerminalStatus;
  pid?: number | null;
  rows: number;
  cols: number;
}

export interface TerminalOutputChunk {
  seq: number;
  data: string;
  timestamp: string;
  stream: 'pty' | string;
}

export interface TerminalPollOutputResponse {
  session_id: string;
  status: TerminalStatus;
  chunks: TerminalOutputChunk[];
  next_seq: number;
}

export interface TerminalWriteResponse {
  session_id: string;
  bytes_written: number;
}

export interface TerminalResizeResponse {
  session_id: string;
  rows: number;
  cols: number;
}

export interface TerminalCloseResponse {
  session_id: string;
  status: TerminalStatus;
}

export interface TerminalSessionSummary extends TerminalSession {
  created_at: string;
  updated_at: string;
}

interface RpcEnvelope<T> {
  result: T;
  logs?: string[];
}

function unwrapRpc<T>(value: T | RpcEnvelope<T>): T {
  if (value && typeof value === 'object' && 'result' in value) {
    return (value as RpcEnvelope<T>).result;
  }
  return value as T;
}

function ensureTauri(): void {
  if (!isTauri()) {
    throw new Error('Not running in Tauri');
  }
}

export async function openhumanTerminalStartSession(
  params: TerminalStartParams
): Promise<TerminalSession> {
  ensureTauri();
  const response = await callCoreRpc<TerminalSession | RpcEnvelope<TerminalSession>>({
    method: 'openhuman.terminal_start_session',
    params,
  });
  return unwrapRpc(response);
}

export async function openhumanTerminalWrite(
  sessionId: string,
  data: string,
  actor = 'user'
): Promise<TerminalWriteResponse> {
  ensureTauri();
  return await callCoreRpc<TerminalWriteResponse>({
    method: 'openhuman.terminal_write',
    params: { session_id: sessionId, data, actor },
  });
}

export async function openhumanTerminalPollOutput(
  sessionId: string,
  afterSeq: number,
  maxChunks = 256
): Promise<TerminalPollOutputResponse> {
  ensureTauri();
  return await callCoreRpc<TerminalPollOutputResponse>({
    method: 'openhuman.terminal_poll_output',
    params: { session_id: sessionId, after_seq: afterSeq, max_chunks: maxChunks },
  });
}

export async function openhumanTerminalResize(
  sessionId: string,
  rows: number,
  cols: number
): Promise<TerminalResizeResponse> {
  ensureTauri();
  return await callCoreRpc<TerminalResizeResponse>({
    method: 'openhuman.terminal_resize',
    params: { session_id: sessionId, rows, cols },
  });
}

export async function openhumanTerminalClose(sessionId: string): Promise<TerminalCloseResponse> {
  ensureTauri();
  return await callCoreRpc<TerminalCloseResponse>({
    method: 'openhuman.terminal_close',
    params: { session_id: sessionId },
  });
}

export async function openhumanTerminalListSessions(): Promise<TerminalSessionSummary[]> {
  ensureTauri();
  const response = await callCoreRpc<{ sessions: TerminalSessionSummary[] }>({
    method: 'openhuman.terminal_list_sessions',
  });
  return response.sessions;
}
