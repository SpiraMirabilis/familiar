"Hands MCP lines (#$#...) to the MCP simple-edit feature, for servers built with OUT_OF_BAND_PREFIX.";
"Replace FEATURE with the feature's object number.";
if (callers())
  return E_PERM;
endif
if (valid(player) && FEATURE in `player.features ! ANY => {}')
  return FEATURE:handle_line(argstr);
endif
