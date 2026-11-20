globalThis.vegaAllocate = async (url, title, colour, ownedIds) => {
  const control = await chrome.tabs.getCurrent();
  if (!chrome.extension.inIncognitoContext || !control?.incognito) {
    throw new Error("Vega requires a project context");
  }

  const tab = await chrome.tabs.create({ windowId: control.windowId, url, active: false });
  try {
    const result = await chrome.runtime.sendMessage({
      tabId: tab.id, ownedIds, title, colour,
    });
    if (!result?.ok) {
      throw new Error(result?.error ?? "Vega grouping worker did not respond");
    }
    return tab.id;
  } catch (error) {
    await chrome.tabs.remove(tab.id);
    throw error;
  }
};
