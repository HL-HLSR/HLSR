// Ghosting Mod: Renewed (Half-Life 2 with HL1 Movement, Episode One, Episode Two) — LiveSplit autosplitter.
// HLSR: https://github.com/HL-HLSR/HLSR
//
// Game time comes straight from the game, not from LiveSplit: the mod keeps a GhLiveSplitBlock
// (static global in client.dll or server.dll) with the exact run time the in-game timer shows,
// so the in-game timer, the .run file, demos and LiveSplit end on the same millisecond.
//
//   offset  0  char[16] magic "GHOSTING-LS-v1\0\0"
//          16  uint32  seq          seqlock: odd while the game writes
//          20  uint32  runId        +1 on every run start
//          24  uint32  state        0 no run, 1 running, 2 finished
//          28  uint32  splitIndex   +1 on every forward level transition
//          32  double  timeSeconds  exact run time
//          40  int32   ticks
//          44  float   tickInterval
//          48  char[64] map
//
// Compare against Game Time. Splits: every level transition (default) or HL2 chapters only.

state("hl2") {}

startup
{
    refreshRate = 120;
    settings.Add("chapters", false, "Split only on Half-Life 2 chapter transitions (otherwise every map)");
    settings.Add("reset", true, "Reset when a new run starts in game");

    // Same transitions as the HLSR SourceSplit presets (MapTransitionList).
    vars.chapterPairs = new HashSet<string> {
        "d1_trainstation_06>d1_canals_01", "d1_trainstation_04>d1_trainstation_05",
        "d1_canals_05>d1_canals_06", "d1_canals_13>d1_eli_01", "d1_eli_02>d1_town_01",
        "d1_town_05>d2_coast_01", "d2_coast_07>d2_coast_09", "d2_prison_01>d2_prison_02",
        "d3_c17_01>d3_c17_02", "d2_prison_05>d2_prison_06", "d3_c17_08>d3_c17_09",
        "d3_c17_13>d3_citadel_01", "d3_citadel_05>d3_breen_01"
    };
    // shortcut: chapter mode knows HL2 only; episodes split on every map until their chapter lists are added.
    vars.skipPairs = new HashSet<string> { "d1_canals_07>d1_canals_08" };
    vars.lastScan = DateTime.MinValue;
    vars.magic = new SigScanTarget(0, "47 48 4F 53 54 49 4E 47 2D 4C 53 2D 76 31 00 00");
}

init
{
    // Without the block (old Ghosting, another hl2.exe game) LiveSplit retries init every tick:
    // rescan the DLLs at most every 3 seconds.
    if ((DateTime.Now - vars.lastScan).TotalSeconds < 3)
        throw new Exception("GhLiveSplitBlock not found yet");
    vars.lastScan = DateTime.Now;
    vars.block = IntPtr.Zero;
    foreach (var m in game.ModulesWow64Safe())
    {
        var name = m.ModuleName.ToLowerInvariant();
        if (name != "client.dll" && name != "server.dll")
            continue;
        var found = new SignatureScanner(game, m.BaseAddress, m.ModuleMemorySize).Scan(vars.magic);
        if (found != IntPtr.Zero)
        {
            vars.block = found;
            break;
        }
    }
    // The mod DLLs load after the engine: LiveSplit retries init until the block shows up.
    if (vars.block == IntPtr.Zero)
        throw new Exception("GhLiveSplitBlock not found yet");
    vars.ready = false;
    vars.pendingStart = false;
    vars.splitFrom = null;
    vars.doSplit = false;
}

update
{
    IntPtr p = vars.block;
    byte[] b = game.ReadBytes(p, 112);
    if (b == null || b.Length < 112)
        return false;
    uint seq = BitConverter.ToUInt32(b, 16);
    if ((seq & 1) != 0 || game.ReadValue<uint>(p + 16) != seq)
        return false;
    int end = Array.IndexOf(b, (byte)0, 48, 64);
    string map = System.Text.Encoding.ASCII.GetString(b, 48, (end < 0 ? 112 : end) - 48);
    uint runId = BitConverter.ToUInt32(b, 20), state = BitConverter.ToUInt32(b, 24), split = BitConverter.ToUInt32(b, 28);
    if (!vars.ready)
    {
        // First read after attaching: remember, fire nothing.
        vars.runId = runId; vars.state = state; vars.split = split; vars.map = map;
        vars.ready = true;
    }
    vars.oldRunId = vars.runId; vars.oldState = vars.state; vars.oldSplit = vars.split; vars.oldMap = vars.map;
    vars.runId = runId; vars.state = state; vars.split = split; vars.map = map;
    vars.time = BitConverter.ToDouble(b, 32);

    // The game counts the transition at trigger_changelevel, the map name changes after the load:
    // decide the split once the destination is known.
    if (vars.runId != vars.oldRunId)
        vars.splitFrom = null;
    else if (vars.split > vars.oldSplit && vars.splitFrom == null)
        vars.splitFrom = vars.oldMap;
    if (vars.splitFrom != null && vars.map != vars.splitFrom)
    {
        string pair = vars.splitFrom + ">" + vars.map;
        vars.doSplit = settings["chapters"] ? vars.chapterPairs.Contains(pair) : !vars.skipPairs.Contains(pair);
        vars.splitFrom = null;
    }

    // LiveSplit runs no reset block once the run has ended: a new in-game run resets from here.
    if (timer.CurrentPhase == TimerPhase.Ended && vars.runId != vars.oldRunId && settings["reset"])
    {
        new TimerModel { CurrentState = timer }.Reset();
        vars.pendingStart = vars.state == 1;
    }
}

start
{
    if (vars.state == 1 && (vars.runId != vars.oldRunId || vars.pendingStart))
    {
        vars.pendingStart = false;
        vars.doSplit = false;
        return true;
    }
    return false;
}

reset
{
    if (!settings["reset"] || vars.runId == vars.oldRunId)
        return false;
    // A new run began: reset now, start on the next update.
    vars.pendingStart = vars.state == 1;
    return true;
}

split
{
    if (vars.state == 2 && vars.oldState != 2)
        return true;
    if (!vars.doSplit)
        return false;
    vars.doSplit = false;
    return true;
}

isLoading
{
    return true;
}

gameTime
{
    // state 0: no run in the game (or it restarted) — keep LiveSplit's time as it is.
    if (vars.state == 0)
        return null;
    return TimeSpan.FromSeconds(vars.time);
}
