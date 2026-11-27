import engine.Engine

## Windows can refuse a refresh after admission if native facts are unavailable.
## The pinned wire has no host-refusal tag, so the host sends the empty Timeout result.
Environment := [].{
	snapshot : {} -> Engine.Ask(Engine.Boot, [Refused])
	snapshot = |_| Engine.ask_of(Refresh.{}, {})
}

Refresh := {}.{
	request : Refresh, {} -> Engine.Request
	request = |_, _| EnvironmentSnapshot

	result : Refresh, Engine.Result -> Try(Engine.Boot, [Refused])
	result = |_, answer| match answer {
		Environment(facts) => Ok(facts)
		_ => Err(Refused)
	}
}
