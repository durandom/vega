async (id) => {
  if (location.href !== 'chrome://extensions/?id=' + id) return false;
  const detail = document.querySelector('extensions-manager')?.shadowRoot
    ?.querySelector('extensions-detail-view');
  if (detail?.data?.id !== id) return false;
  if (detail.data.incognitoAccess?.isActive && detail.data.state === 'ENABLED') return true;

  const toggle = detail.shadowRoot?.querySelector('#allow-incognito')
    ?.shadowRoot?.querySelector('input[type=checkbox]');
  if (toggle && !toggle.disabled && !toggle.checked && !globalThis.vegaIncognitoToggleClicked) {
    // Changing the grant reloads the unpacked extension. Chrome disables it
    // during that reload if Developer mode is off.
    await chrome.developerPrivate.updateProfileConfiguration({ inDeveloperMode: true });
    if (toggle.isConnected && !toggle.checked) {
      globalThis.vegaIncognitoToggleClicked = true;
      toggle.click();
    }
  }
  return false;
}
