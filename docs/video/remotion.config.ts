import {Config} from '@remotion/cli/config';

// The scenes use the README screenshots directly (made by firmware/sim/shots/screenshots.py).
Config.setPublicDir('../screenshots');
Config.setVideoImageFormat('jpeg');
Config.setJpegQuality(95);
