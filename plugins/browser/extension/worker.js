async function groupTabs({ tabId, ownedIds, title, colour }, control) {
  const owned = new Set([...ownedIds, tabId]);
  const tabs = await chrome.tabs.query({ windowId: control.windowId });
  const members = tabs.filter(tab => owned.has(tab.id));
  if (!members.some(tab => tab.id === tabId) || members.some(tab => !tab.incognito)) {
    throw new Error("Vega tab is outside the control window");
  }

  // Chrome keeps group membership across worker restarts. Recover a group
  // from verified tab IDs and never take over a group that contains other tabs.
  const group = members.find(tab => tab.groupId >= 0 &&
    tabs.every(other => other.groupId !== tab.groupId || owned.has(other.id)));
  const tabIds = members.map(tab => tab.id);
  const groupId = group
    ? await chrome.tabs.group({ tabIds, groupId: group.groupId })
    : await chrome.tabs.group({ tabIds, createProperties: { windowId: control.windowId } });
  await chrome.tabGroups.update(groupId, { title, color: colour });
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (!chrome.extension.inIncognitoContext || !sender.tab?.incognito ||
      sender.url !== chrome.runtime.getURL("control.html")) {
    return false;
  }
  groupTabs(message, sender.tab).then(
    () => respond({ ok: true }),
    error => respond({ error: error.message }),
  );
  return true;
});
