import {Composition} from 'remotion';
import {Tour, TOUR_FRAMES} from './Tour';

export const RemotionRoot = () => (
  <Composition id="Tour" component={Tour} durationInFrames={TOUR_FRAMES} fps={30} width={1920} height={1080} />
);
