import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const terminalMocks = vi.hoisted(() => ({
  start: vi.fn(),
  write: vi.fn(),
  poll: vi.fn(),
  resize: vi.fn(),
  close: vi.fn(),
}));

const xtermMocks = vi.hoisted(() => {
  class MockTerminal {
    static instances: MockTerminal[] = [];
    rows = 24;
    cols = 80;
    writes: string[] = [];
    onDataHandler: ((data: string) => void) | null = null;
    loadAddon = vi.fn();
    open = vi.fn();
    writeln = vi.fn((data: string) => this.writes.push(`${data}\n`));
    write = vi.fn((data: string) => this.writes.push(data));
    clear = vi.fn();
    focus = vi.fn();
    dispose = vi.fn();
    onData = vi.fn((handler: (data: string) => void) => {
      this.onDataHandler = handler;
      return { dispose: vi.fn() };
    });
    constructor() {
      MockTerminal.instances.push(this);
    }
  }

  class MockFitAddon {
    fit = vi.fn();
  }

  return { MockTerminal, MockFitAddon };
});

vi.mock('@xterm/xterm', () => ({ Terminal: xtermMocks.MockTerminal }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: xtermMocks.MockFitAddon }));

vi.mock('../../../../lib/i18n/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key, locale: 'en', setLocale: vi.fn() }),
}));

vi.mock('../../hooks/useSettingsNavigation', () => ({
  useSettingsNavigation: () => ({ navigateBack: vi.fn(), breadcrumbs: [] }),
}));

vi.mock('../../../../utils/tauriCommands/terminal', () => ({
  openhumanTerminalStartSession: terminalMocks.start,
  openhumanTerminalWrite: terminalMocks.write,
  openhumanTerminalPollOutput: terminalMocks.poll,
  openhumanTerminalResize: terminalMocks.resize,
  openhumanTerminalClose: terminalMocks.close,
}));

vi.mock('../components/SettingsHeader', () => ({ default: () => null }));

describe('TerminalPanel', () => {
  beforeEach(() => {
    xtermMocks.MockTerminal.instances.length = 0;
    Object.values(terminalMocks).forEach(mock => mock.mockReset());
    terminalMocks.start.mockResolvedValue({
      session_id: 'term-1',
      kind: 'local',
      status: 'running',
      pid: 123,
      rows: 24,
      cols: 80,
    });
    terminalMocks.write.mockResolvedValue({ session_id: 'term-1', bytes_written: 1 });
    terminalMocks.poll.mockResolvedValue({
      session_id: 'term-1',
      status: 'running',
      chunks: [],
      next_seq: 0,
    });
    terminalMocks.resize.mockResolvedValue({ session_id: 'term-1', rows: 24, cols: 80 });
    terminalMocks.close.mockResolvedValue({ session_id: 'term-1', status: 'closed' });
    (globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
      observe = vi.fn();
      disconnect = vi.fn();
    } as unknown as typeof ResizeObserver;
  });

  it('starts a local PTY session with approval', async () => {
    const { default: TerminalPanel } = await import('../TerminalPanel');
    render(<TerminalPanel />);

    fireEvent.change(screen.getByLabelText('terminal.command'), {
      target: { value: 'python' },
    });
    fireEvent.click(screen.getByLabelText('terminal.approvalHint'));
    fireEvent.click(screen.getByRole('button', { name: 'terminal.start' }));

    await waitFor(() => expect(terminalMocks.start).toHaveBeenCalled());
    expect(terminalMocks.start).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: 'local',
        command: 'python',
        approved: true,
      })
    );
  });

  it('writes user data into the active PTY session', async () => {
    const { default: TerminalPanel } = await import('../TerminalPanel');
    render(<TerminalPanel />);

    fireEvent.click(screen.getByLabelText('terminal.approvalHint'));
    fireEvent.click(screen.getByRole('button', { name: 'terminal.start' }));
    await waitFor(() => expect(terminalMocks.start).toHaveBeenCalled());

    xtermMocks.MockTerminal.instances[0].onDataHandler?.('x');
    await waitFor(() => expect(terminalMocks.write).toHaveBeenCalledWith('term-1', 'x', 'user'));
  });
});
