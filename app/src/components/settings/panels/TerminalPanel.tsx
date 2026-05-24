import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import debug from 'debug';
import { useCallback, useEffect, useRef, useState } from 'react';

import { useT } from '../../../lib/i18n/I18nContext';
import {
  openhumanTerminalClose,
  openhumanTerminalPollOutput,
  openhumanTerminalResize,
  openhumanTerminalStartSession,
  openhumanTerminalWrite,
} from '../../../utils/tauriCommands/terminal';
import type {
  TerminalKind,
  TerminalSession,
  TerminalStatus,
} from '../../../utils/tauriCommands/terminal';
import SettingsHeader from '../components/SettingsHeader';
import { useSettingsNavigation } from '../hooks/useSettingsNavigation';

const log = debug('app:settings:terminal');
const POLL_INTERVAL_MS = 250;

const statusKey = (status: TerminalStatus): string => `terminal.status.${status}`;

const TerminalPanel = () => {
  const { t } = useT();
  const { navigateBack } = useSettingsNavigation();
  const containerRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sessionRef = useRef<TerminalSession | null>(null);
  const statusRef = useRef<TerminalStatus>('closed');
  const nextSeqRef = useRef(0);
  const [kind, setKind] = useState<TerminalKind>('local');
  const [host, setHost] = useState('');
  const [user, setUser] = useState('');
  const [port, setPort] = useState('22');
  const [command, setCommand] = useState('');
  const [approved, setApproved] = useState(false);
  const [session, setSession] = useState<TerminalSession | null>(null);
  const [status, setStatus] = useState<TerminalStatus>('closed');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const welcomeText = t('terminal.welcome');

  const fitAndResize = useCallback(() => {
    const fit = fitRef.current;
    const term = terminalRef.current;
    const current = sessionRef.current;
    if (!fit || !term) return;
    try {
      fit.fit();
      if (current) {
        void openhumanTerminalResize(current.session_id, term.rows, term.cols).catch(err => {
          log('resize failed %o', err);
        });
      }
    } catch (err) {
      log('fit failed %o', err);
    }
  }, []);

  useEffect(() => {
    const term = new Terminal({
      cursorBlink: true,
      convertEol: true,
      fontFamily: 'JetBrains Mono, ui-monospace, SFMono-Regular, Menlo, monospace',
      fontSize: 13,
      theme: {
        background: '#0f172a',
        foreground: '#e5e7eb',
        cursor: '#93c5fd',
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    terminalRef.current = term;
    fitRef.current = fit;

    if (containerRef.current) {
      term.open(containerRef.current);
      term.writeln(welcomeText);
      fitAndResize();
    }

    const dataDisposable = term.onData(data => {
      const current = sessionRef.current;
      if (!current || statusRef.current === 'closed') return;
      void openhumanTerminalWrite(current.session_id, data, 'user').catch(err => {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
        log('write failed %s', message);
      });
    });
    const resizeObserver = new ResizeObserver(fitAndResize);
    if (containerRef.current) {
      resizeObserver.observe(containerRef.current);
    }

    return () => {
      dataDisposable.dispose();
      resizeObserver.disconnect();
      term.dispose();
      terminalRef.current = null;
      fitRef.current = null;
    };
  }, [fitAndResize, welcomeText]);

  useEffect(() => {
    sessionRef.current = session;
  }, [session]);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  useEffect(() => {
    if (!session) return;
    let cancelled = false;
    const interval = window.setInterval(() => {
      void openhumanTerminalPollOutput(session.session_id, nextSeqRef.current).then(
        response => {
          if (cancelled) return;
          nextSeqRef.current = response.next_seq;
          setStatus(response.status);
          for (const chunk of response.chunks) {
            terminalRef.current?.write(chunk.data);
          }
          if (response.status !== 'running') {
            log('session status changed %s', response.status);
          }
        },
        err => {
          if (cancelled) return;
          const message = err instanceof Error ? err.message : String(err);
          setError(message);
          log('poll failed %s', message);
        }
      );
    }, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [session]);

  const startSession = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      terminalRef.current?.clear();
      terminalRef.current?.writeln(t('terminal.starting'));
      const started = await openhumanTerminalStartSession({
        kind,
        command: kind === 'local' && command.trim() ? command.trim() : null,
        rows: terminalRef.current?.rows,
        cols: terminalRef.current?.cols,
        approved,
        actor: 'user',
        ssh:
          kind === 'ssh'
            ? {
                host: host.trim(),
                user: user.trim() || null,
                port: Number(port) || 22,
                request_tty: true,
              }
            : null,
      });
      nextSeqRef.current = 0;
      setSession(started);
      setStatus(started.status);
      terminalRef.current?.focus();
      log('started terminal session %s', started.session_id);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      terminalRef.current?.writeln(`\r\n${t('terminal.errorPrefix')} ${message}\r\n`);
    } finally {
      setBusy(false);
    }
  }, [approved, command, host, kind, port, t, user]);

  const closeSession = useCallback(async () => {
    const current = sessionRef.current;
    if (!current) return;
    setBusy(true);
    setError(null);
    try {
      await openhumanTerminalClose(current.session_id);
      setSession(null);
      sessionRef.current = null;
      setStatus('closed');
      terminalRef.current?.writeln(`\r\n${t('terminal.closed')}\r\n`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [t]);

  const isRunning = Boolean(session && status === 'running');

  return (
    <div data-testid="terminal-panel">
      <SettingsHeader
        title={t('terminal.title')}
        description={t('terminal.description')}
        onBack={navigateBack}
      />

      <div className="px-4 pb-4 space-y-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="text-xs font-medium text-stone-700 dark:text-neutral-300">
            {t('terminal.kind')}
            <select
              className="mt-1 w-full rounded-lg border border-stone-200 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
              value={kind}
              disabled={isRunning || busy}
              onChange={event => setKind(event.target.value as TerminalKind)}>
              <option value="local">{t('terminal.kind.local')}</option>
              <option value="ssh">{t('terminal.kind.ssh')}</option>
            </select>
          </label>

          <label className="text-xs font-medium text-stone-700 dark:text-neutral-300">
            {t('terminal.command')}
            <input
              className="mt-1 w-full rounded-lg border border-stone-200 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
              value={command}
              disabled={kind !== 'local' || isRunning || busy}
              placeholder={t('terminal.commandPlaceholder')}
              onChange={event => setCommand(event.target.value)}
            />
          </label>
        </div>

        {kind === 'ssh' && (
          <div className="grid gap-3 sm:grid-cols-[1fr_1fr_96px]">
            <label className="text-xs font-medium text-stone-700 dark:text-neutral-300">
              {t('terminal.sshHost')}
              <input
                className="mt-1 w-full rounded-lg border border-stone-200 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
                value={host}
                disabled={isRunning || busy}
                placeholder={t('terminal.sshHostPlaceholder')}
                onChange={event => setHost(event.target.value)}
              />
            </label>
            <label className="text-xs font-medium text-stone-700 dark:text-neutral-300">
              {t('terminal.sshUser')}
              <input
                className="mt-1 w-full rounded-lg border border-stone-200 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
                value={user}
                disabled={isRunning || busy}
                placeholder={t('terminal.sshUserPlaceholder')}
                onChange={event => setUser(event.target.value)}
              />
            </label>
            <label className="text-xs font-medium text-stone-700 dark:text-neutral-300">
              {t('terminal.sshPort')}
              <input
                className="mt-1 w-full rounded-lg border border-stone-200 bg-white px-3 py-2 text-sm dark:border-neutral-700 dark:bg-neutral-900"
                value={port}
                disabled={isRunning || busy}
                inputMode="numeric"
                onChange={event => setPort(event.target.value)}
              />
            </label>
          </div>
        )}

        <label className="flex items-start gap-2 text-xs text-stone-600 dark:text-neutral-300">
          <input
            type="checkbox"
            className="mt-0.5 rounded border-stone-300 text-primary-600"
            checked={approved}
            disabled={isRunning || busy}
            onChange={event => setApproved(event.target.checked)}
          />
          <span>{t('terminal.approvalHint')}</span>
        </label>

        <div className="flex items-center justify-between gap-3">
          <div className="text-xs text-stone-500 dark:text-neutral-400">
            {t('terminal.status')}: <span className="font-medium">{t(statusKey(status))}</span>
            {session?.pid ? (
              <span>
                {' '}
                · {t('terminal.pid')} {session.pid}
              </span>
            ) : null}
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              className="rounded-lg bg-primary-600 px-3 py-2 text-sm font-medium text-white disabled:opacity-50"
              disabled={busy || isRunning || (kind === 'ssh' && host.trim().length === 0)}
              onClick={() => void startSession()}>
              {busy ? t('terminal.startingButton') : t('terminal.start')}
            </button>
            <button
              type="button"
              className="rounded-lg border border-stone-200 px-3 py-2 text-sm font-medium text-stone-700 disabled:opacity-50 dark:border-neutral-700 dark:text-neutral-200"
              disabled={busy || !session}
              onClick={() => void closeSession()}>
              {t('terminal.close')}
            </button>
          </div>
        </div>

        {error && (
          <div className="rounded-lg border border-coral-300 bg-coral-50 px-3 py-2 text-sm text-coral-900 dark:border-coral-500/40 dark:bg-coral-500/10 dark:text-coral-200">
            {error}
          </div>
        )}

        <div
          ref={containerRef}
          className="h-[520px] overflow-hidden rounded-xl border border-neutral-800 bg-slate-950 p-2"
        />
      </div>
    </div>
  );
};

export default TerminalPanel;
