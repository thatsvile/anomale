"use strict";

const HOST = "vilefox";

function applyTheme(message) {
  if (!message || typeof message !== "object") {
    return;
  }
  const colors = message.colors;
  if (!colors || typeof colors !== "object") {
    return;
  }
  browser.theme.update({ colors }).catch((err) => {
    console.error("vilefox theme.update failed", err);
  });
}

function connect() {
  let port;
  try {
    port = browser.runtime.connectNative(HOST);
  } catch (err) {
    console.error("vilefox native connect failed", err);
    setTimeout(connect, 2000);
    return;
  }
  port.onMessage.addListener(applyTheme);
  port.onDisconnect.addListener(() => {
    const err = browser.runtime.lastError;
    if (err) {
      console.error("vilefox native disconnect", err.message);
    }
    setTimeout(connect, 1500);
  });
}

connect();
