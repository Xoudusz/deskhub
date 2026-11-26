# Tour video

The video at the top of the main README, made with [Remotion](https://www.remotion.dev) from the
simulator screenshots in `docs/screenshots` (`firmware/sim/shots/screenshots.py`). Each scene shows one
page in a board frame with a caption (`src/Tour.tsx`: `SHOTS` holds the pages and captions).

    npm ci
    npm run render     # out/deskhub.mp4, 1920x1080, 30 fps
    npm run studio     # live preview in the browser

After new screenshots, render again and upload `out/deskhub.mp4` by dragging it into a README edit on
GitHub; that gives the `user-attachments` link the README uses. Remotion is free for individuals and
small companies (see its license).
