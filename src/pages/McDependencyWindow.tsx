import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import * as shell from '@tauri-apps/plugin-shell';
import { ExternalLink, Moon, Sun } from 'lucide-react';
import { Button, IconButton } from '../components';
import { useTranslation } from 'react-i18next';

import './McDependencyWindow.css';

type MissingUwpDependency = {
  name: string;
  pfn: string;
  min_version: string | null;
};

type McDepsPrompt = {
  title: string;
  main: string;
  content: string;
  install_button: string;
  exit_button: string;
  missing: MissingUwpDependency[];
};

type McDepsProgress = {
  percent: number;
  stage: string;
};

type McDepsLog = {
  key: string;
  name?: string | null;
  pkg?: string | null;
};

type McDepsDone = {
  ok: boolean;
  message: string;
};

const THIRD_PARTY_URL = 'https://www.mcappx.com/download/mc-framework/';

export default function McDependencyWindow() {
  const { t } = useTranslation();
  const [prompt, setPrompt] = useState<McDepsPrompt | null>(null);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<McDepsProgress | null>(null);
  const [done, setDone] = useState<McDepsDone | null>(null);
  const [logs, setLogs] = useState<string[]>([]);
  const [restartIn, setRestartIn] = useState<number | null>(null);

  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    const savedTheme = localStorage.getItem('app-theme');
    if (savedTheme === 'light' || savedTheme === 'dark') return savedTheme;
    if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
      return 'dark';
    }
    return 'light';
  });

  const missingText = useMemo(() => {
    if (!prompt) return '';
    return prompt.missing
      .map((d) => (d.min_version ? `${d.name} (min ${d.min_version})` : d.name))
      .join('\n');
  }, [prompt]);

  const contentText = useMemo(() => {
    return t('McDeps.content', { defaultValue: prompt?.content ?? '' });
  }, [prompt, t]);

  const showContent = useMemo(() => {
    const c = contentText.trim();
    if (!c) return false;
    return c !== missingText.trim();
  }, [contentText, missingText]);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  useEffect(() => {
    const unlistenFns: Array<() => void> = [];

    (async () => {
      const w = getCurrentWindow();
      const unlistenClose = await w.onCloseRequested(async (e) => {
        e.preventDefault();
        await invoke('quit_app');
      });
      unlistenFns.push(unlistenClose);

      const p = await invoke<McDepsPrompt>('get_mc_deps_prompt');
      setPrompt(p);
      document.title = p.title;

      if (!p.missing || p.missing.length === 0) {
        await w.close();
        return;
      }

      const unlistenLog = await listen<McDepsLog>('mc-deps-log', (event) => {
        const payload = event.payload;
        const text = t(`McDeps.logs.${payload.key}`, {
          name: payload.name ?? '',
          pkg: payload.pkg ?? '',
          defaultValue: payload.key,
        });
        setLogs((prev) => [...prev, text]);
      });
      const unlistenProgress = await listen<McDepsProgress>(
        'mc-deps-progress',
        (event) => setProgress(event.payload)
      );
      const unlistenDone = await listen<McDepsDone>('mc-deps-done', (event) => {
        setDone(event.payload);
        setInstalling(false);
        if (event.payload.ok && !event.payload.message) {
          setRestartIn(3);
        }
      });

      unlistenFns.push(unlistenLog, unlistenProgress, unlistenDone);
    })().catch((e) => {
      setLogs((prev) => [
        ...prev,
        t('McDeps.errors.initFailed', { message: String(e) }),
      ]);
    });

    return () => {
      for (const fn of unlistenFns) fn();
    };
  }, []);

  useEffect(() => {
    if (restartIn == null) return;
    if (restartIn <= 0) {
      invoke('restart_app').catch(() => {});
      return;
    }
    const tmr = window.setTimeout(() => setRestartIn((s) => (s == null ? null : s - 1)), 1000);
    return () => window.clearTimeout(tmr);
  }, [restartIn]);

  const onInstall = async () => {
    setInstalling(true);
    setDone(null);
    setRestartIn(null);
    try {
      await invoke('start_mc_deps_install');
    } catch (e) {
      setInstalling(false);
      setLogs((prev) => [
        ...prev,
        t('McDeps.errors.startFailed', { message: String(e) }),
      ]);
    }
  };

  const onExit = async () => {
    await invoke('quit_app');
  };

  const onOpenStore = useCallback(async () => {
    const pfn = prompt?.missing?.[0]?.pfn;
    if (!pfn) return;
    await invoke('open_ms_store_for_pfn', { pfn }).catch(() => {});
  }, [prompt]);

  const onThirdParty = useCallback(async () => {
    await shell.open(THIRD_PARTY_URL).catch(() => {});
  }, []);

  const toggleTheme = useCallback(() => {
    setTheme((prev) => {
      const next = prev === 'light' ? 'dark' : 'light';
      localStorage.setItem('app-theme', next);
      return next;
    });
  }, []);

  const percent = Math.max(0, Math.min(100, progress?.percent ?? 0));
  const stageLabel = progress
    ? t(`McDeps.stages.${progress.stage}`, {
        defaultValue: progress.stage,
      })
    : t('McDeps.waitingAction');

  return (
    <div className="mcdeps-root">
      <div className="mcdeps-titlebar" data-tauri-drag-region>
        <div className="mcdeps-title">
          {t('McDeps.title', { defaultValue: prompt?.title ?? '...' })}
        </div>
        <div className="mcdeps-titlebar-right">
          <IconButton
            title={t('McDeps.themeToggle')}
            onClick={toggleTheme}
            icon={theme === 'dark' ? <Sun size={18} /> : <Moon size={18} />}
          />
        </div>
      </div>

      <div className="mcdeps-body">
        <h1 className="mcdeps-main">
          {t('McDeps.main', { defaultValue: prompt?.main ?? '' })}
        </h1>
        {showContent && <div className="mcdeps-content">{contentText}</div>}

        <div className="mcdeps-warning">
          {t('McDeps.warning')}
        </div>

        <div className="mcdeps-missing">
          <div style={{ fontWeight: 700, marginBottom: 6 }}>
            {t('McDeps.missingTitle')}
          </div>
          <div style={{ whiteSpace: 'pre-wrap', opacity: 0.9 }}>{missingText}</div>
        </div>

        <div className="mcdeps-progress">
          <div style={{ width: `${percent}%` }} />
        </div>
        <div style={{ fontSize: 12, opacity: 0.85 }}>
          {t('McDeps.progress', { percent, stage: stageLabel })}
        </div>

        {done && (
          <div className={done.ok ? 'mcdeps-done-ok' : 'mcdeps-done-fail'}>
            {done.ok
              ? restartIn != null
                ? t('McDeps.done.restarting', { seconds: restartIn })
                : t('McDeps.done.ok')
              : t('McDeps.done.fail', { message: done.message })}
          </div>
        )}

        <div className="mcdeps-actions">
          <Button disabled={installing} onClick={onInstall}>
            {installing
              ? t('McDeps.installing')
              : t('McDeps.install', { defaultValue: prompt?.install_button ?? '' })}
          </Button>
          <Button type="secondary" onClick={onOpenStore}>
            {t('McDeps.openStore')}
          </Button>
          <Button type="secondary" onClick={onThirdParty}>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              <ExternalLink size={16} />
              {t('McDeps.thirdParty')}
            </span>
          </Button>
          <Button type="secondary" onClick={onExit}>
            {t('McDeps.exit', { defaultValue: prompt?.exit_button ?? '' })}
          </Button>
        </div>

        <div className="mcdeps-logs">
          {(logs.length ? logs : [t('McDeps.noLogs')]).join('\n')}
        </div>
      </div>
    </div>
  );
}
