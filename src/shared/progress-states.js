const BUSY_STATES = [
  "preparing",
  "downloading",
  "verifying",
  "extracting",
  "finalizing",
  "applying-update",
  "checking-files",
  "repairing",
  "exporting",
  "importing",
];

const PROGRESS_STATES = BUSY_STATES.concat(["paused"]);

const SUCCESS_STATES = ["ready", "files-valid", "repair-complete"];

function isBusy(state) {
  return BUSY_STATES.indexOf(state) !== -1;
}

function isProgress(state) {
  return PROGRESS_STATES.indexOf(state) !== -1;
}

function isSuccess(state) {
  return SUCCESS_STATES.indexOf(state) !== -1;
}

function shouldHoldFinish(previous, next) {
  if (!previous || !next) return false;
  if (!previous.gameId || !next.gameId) return false;
  if (previous.gameId !== next.gameId) return false;

  return isSuccess(next.state) && isProgress(previous.state);
}

module.exports = {
  BUSY_STATES,
  PROGRESS_STATES,
  SUCCESS_STATES,
  isBusy,
  isProgress,
  isSuccess,
  shouldHoldFinish,
};
