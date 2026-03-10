-- crates/server/src/hostexec/default_scripts/gh.lua
-- Default gh argument transform: passthrough (no blocked flags)

function transform(command, args, working_dir)
    return args
end
