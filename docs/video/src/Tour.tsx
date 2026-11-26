import {loadFont} from '@remotion/google-fonts/Inter';
import {AbsoluteFill, Easing, Img, interpolate, Sequence, spring, staticFile, useCurrentFrame, useVideoConfig} from 'remotion';

const {fontFamily} = loadFont('normal', {weights: ['400', '600', '700'], subsets: ['latin', 'latin-ext']});

const C = {bg: '#0a0c0f', text: '#f2f4f7', muted: '#9aa3ad', accent: '#ffb020', bezel: '#16191e'};

export const INTRO = 90;
export const SCENE = 84;
export const OUTRO = 120;
export const FADE = 15;

/** One page: screenshot name (docs/screenshots), caption, and an optional zoom towards `focus`. */
export type Shot = {file: string; title: string; line: string; zoom?: number; focus?: string};

export const SHOTS: Shot[] = [
  {file: 'home', title: 'Home', line: 'Your day at a glance: limits, next events, weather, music and PC load.'},
  {file: 'alert', title: 'Allow or deny', line: 'Claude Code asks for a permission. Answer it from the board.', zoom: 1.12, focus: '30% 20%'},
  {file: 'stats', title: 'PC stats', line: 'CPU, GPU, RAM and VRAM, with a one-minute history.'},
  {file: 'network', title: 'Internet', line: 'Latency, drops, data used and a speed test.'},
  {file: 'ai', title: 'Claude Code usage', line: 'Five-hour and weekly limits, tokens and models.'},
  {file: 'calendar', title: 'Calendar', line: 'Outlook, Google or any .ics link.'},
  {file: 'weather', title: 'Weather', line: 'Now, the next 24 hours and 6 days. Works with the PC off.'},
  {file: 'media', title: 'Now playing', line: 'Spotify, YouTube, VLC… with the cover and controls.'},
  {file: 'plex', title: 'Plex', line: 'What plays on your server, and who is watching.'},
  {file: 'shortcuts', title: 'Shortcuts', line: 'One tap for keys, apps, links, volume and more.'},
  {file: 'games', title: 'Games', line: 'Your Steam game, session time and recent games.'},
  {file: 'github', title: 'GitHub', line: 'A year of contributions, streaks and top repos.'},
  {file: 'timer', title: 'Timer', line: 'A focus timer with presets. The P4 board rings.'},
];

export const TOUR_FRAMES = INTRO + SHOTS.length * SCENE + OUTRO - (SHOTS.length + 1) * FADE;


// The board: 800x480 screen at 1.375x in a dark bezel, right of the caption. The bezel stays put;
// only the screen changes, the way the real board switches pages.
const SCREEN_W = 1100;
const SCREEN_H = 660;
const BEZEL = 28;
const BOARD_LEFT = 690;
const BOARD_TOP = (1080 - SCREEN_H - 2 * BEZEL) / 2;

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const Bezel = () => {
  const frame = useCurrentFrame();
  const end = INTRO - FADE + SHOTS.length * (SCENE - FADE) + FADE;
  const o = interpolate(frame, [INTRO - FADE, INTRO, end - FADE, end], [0, 1, 1, 0], clamp);
  return (
    <div
      style={{
        position: 'absolute',
        left: BOARD_LEFT,
        top: BOARD_TOP,
        width: SCREEN_W + 2 * BEZEL,
        height: SCREEN_H + 2 * BEZEL,
        borderRadius: 40,
        background: C.bezel,
        boxShadow: '0 40px 120px rgba(0,0,0,0.55), inset 0 0 0 2px #23272e',
        opacity: o,
      }}
    />
  );
};

/** One page: its screen fades in over the last one, its caption after the last caption is gone. */
const Page = ({shot}: {shot: Shot}) => {
  const frame = useCurrentFrame();
  const {fps, durationInFrames} = useVideoConfig();
  // Pages hold still like the real screen; a zoomed page is back at 1x before the next fades in.
  const zoom = shot.zoom ?? 1;
  const d = durationInFrames;
  const scale = interpolate(frame, [FADE, 0.4 * d, d - 2 * FADE, d - FADE], [1, zoom, zoom, 1], {easing: Easing.inOut(Easing.quad), ...clamp});
  const screen = interpolate(frame, [0, FADE], [0, 1], clamp);
  const out = interpolate(frame, [durationInFrames - FADE, durationInFrames - 3], [1, 0], clamp);
  const t = spring({frame: frame - FADE, fps, config: {damping: 200}});
  const l = spring({frame: frame - FADE - 6, fps, config: {damping: 200}});
  return (
    <AbsoluteFill style={{fontFamily}}>
      <div style={{position: 'absolute', left: 120, top: 0, bottom: 0, width: 500, display: 'flex', flexDirection: 'column', justifyContent: 'center', opacity: out}}>
        <div style={{width: 56, height: 6, borderRadius: 3, background: C.accent, marginBottom: 32, opacity: t}} />
        <div style={{fontSize: 76, fontWeight: 700, lineHeight: 1.05, color: C.text, opacity: t, transform: `translateY(${(1 - t) * 24}px)`}}>
          {shot.title}
        </div>
        <div style={{fontSize: 38, lineHeight: 1.35, color: C.muted, marginTop: 28, opacity: l, transform: `translateY(${(1 - l) * 24}px)`}}>
          {shot.line}
        </div>
      </div>
      <div style={{position: 'absolute', left: BOARD_LEFT + BEZEL, top: BOARD_TOP + BEZEL, width: SCREEN_W, height: SCREEN_H, overflow: 'hidden', borderRadius: 8, opacity: screen}}>
        <Img
          src={staticFile(`${shot.file}.png`)}
          style={{width: SCREEN_W, height: SCREEN_H, transform: `scale(${scale})`, transformOrigin: shot.focus ?? '50% 50%'}}
        />
      </div>
    </AbsoluteFill>
  );
};

const Title = ({sub, small}: {sub: string; small: string}) => {
  const frame = useCurrentFrame();
  const {fps, durationInFrames} = useVideoConfig();
  const a = spring({frame, fps, config: {damping: 200}});
  const b = spring({frame: frame - 10, fps, config: {damping: 200}});
  const o = interpolate(frame, [durationInFrames - FADE, durationInFrames], [1, 0], clamp);
  return (
    <AbsoluteFill style={{fontFamily, alignItems: 'center', justifyContent: 'center', textAlign: 'center', opacity: o}}>
      <div style={{fontSize: 150, fontWeight: 700, color: C.text, letterSpacing: -3, opacity: a, transform: `scale(${0.96 + 0.04 * a})`}}>
        Desk <span style={{color: C.accent}}>Hub</span>
      </div>
      <div style={{fontSize: 48, color: C.text, marginTop: 24, opacity: b}}>{sub}</div>
      <div style={{fontSize: 34, color: C.muted, marginTop: 20, opacity: b}}>{small}</div>
    </AbsoluteFill>
  );
};

/** Scenes overlap by FADE frames: intro, one per page, outro. */
export const Tour = () => {
  const pageAt = (i: number) => INTRO - FADE + i * (SCENE - FADE);
  const outroAt = pageAt(SHOTS.length);
  return (
    <AbsoluteFill style={{background: C.bg}}>
      <Bezel />
      <Sequence name="Intro" durationInFrames={INTRO}>
        <Title sub="A desk display for your PC" small="ESP32-S3 4.3B or ESP32-P4 4.3  ·  Windows desktop app" />
      </Sequence>
      {SHOTS.map((shot, i) => (
        <Sequence key={shot.file} name={shot.title} from={pageAt(i)} durationInFrames={SCENE}>
          <Page shot={shot} />
        </Sequence>
      ))}
      <Sequence name="Outro" from={outroAt} durationInFrames={OUTRO}>
        <Title sub="Open source on GitHub" small="github.com/Xoudusz/deskhub" />
      </Sequence>
    </AbsoluteFill>
  );
};
