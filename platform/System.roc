import engine.Port
import Engine

## Native Windows clocks, regional presentation, and secure entropy.
System := [].{
	Failure : [Unavailable, Refused]
	FormatFailure : [Unavailable, Refused, TooLarge, OutsideRange]
	RandomFailure : [Unavailable, Refused, TooLarge]

	## An elapsed-time mark valid within the current app process.
	Mark :: U64.{
		is_eq : Mark, Mark -> Bool
		is_eq = |Mark.(first), Mark.(second)| first == second

		elapsed_ms_since : Mark, Mark -> Try(U64, [OutOfOrder])
		elapsed_ms_since = |Mark.(later), Mark.(earlier)|
			if (later < earlier) Err(OutOfOrder) else Ok(later - earlier)
	}

	RegionalSettings : { format_locale : Str, zone : [WindowsZone(Str), UnnamedZone] }
	FormattedTime : { date : Str, time : Str }
	TimestampProps := { zone : [Local, Utc] ?? Local, date : [ShortDate, LongDate] ?? ShortDate }

	monotonic : {} -> Engine.Ask(Mark, Failure)
	monotonic = |_| Engine.ask_of(Monotonic.{}, {})

	regional_settings : {} -> Engine.Ask(RegionalSettings, Failure)
	regional_settings = |_| Engine.ask_of(Regional.{}, {})

	## Format up to 64 signed Unix millisecond values in Windows regional settings.
	format_timestamps : List(I64), TimestampProps -> Engine.Ask(List(FormattedTime), FormatFailure)
	format_timestamps = |instants, { zone, date }| Engine.ask_of(FormatTimes.{}, { instants, zone, date })

	## Draw up to 65,536 fresh bytes from Windows CNG. Zero yields an empty list.
	secure_random : U64 -> Engine.Ask(List(U8), RandomFailure)
	secure_random = |count| Engine.ask_of(Random.{}, count)

	ports : {} -> List(Engine.PortRegistration)
	ports = |_| Port.registrations({ format, monotonic: mark, regional })
}

mark : Port.Call({}, { code : U8, ticks : U64 })
mark = Port.call({ name: "roc-native-windows/monotonic", timeout: Ms(15000) })

regional : Port.Call({}, { code : U8, locale : Str, zone : Str })
regional = Port.call({ name: "roc-native-windows/regional", timeout: Ms(15000) })

FormatInput : { instants : List(I64), zone : [Local, Utc], date : [ShortDate, LongDate] }

format : Port.Call(FormatInput, { code : U8, parts : List(System.FormattedTime) })
format = Port.call({ name: "roc-native-windows/format-time", timeout: Ms(15000) })

Monotonic := {}.{
	request : Monotonic, {} -> Engine.Request
	request = |_, input| mark.request(input)

	result : Monotonic, Engine.Result -> Try(System.Mark, System.Failure)
	result = |_, answer| match answer {
		PortAnswer(_) => match mark.result(answer) {
			Ok({ code: 0, ticks }) => Ok(System.Mark.(ticks))
			Ok({ code: 2, .. }) => Err(Refused)
			_ => Err(Unavailable)
		}
		Refused | PortRefused(_) => Err(Refused)
		_ => Err(Unavailable)
	}
}

Regional := {}.{
	request : Regional, {} -> Engine.Request
	request = |_, input| regional.request(input)

	result : Regional, Engine.Result -> Try(System.RegionalSettings, System.Failure)
	result = |_, answer| match answer {
		PortAnswer(_) => match regional.result(answer) {
			Ok({ code: 0, locale, zone }) => Ok({ format_locale: locale, zone: if (zone.is_empty()) UnnamedZone else WindowsZone(zone) })
			Ok({ code: 2, .. }) => Err(Refused)
			_ => Err(Unavailable)
		}
		Refused | PortRefused(_) => Err(Refused)
		_ => Err(Unavailable)
	}
}

FormatTimes := {}.{
	request : FormatTimes, FormatInput -> Engine.Request
	request = |_, input| if (input.instants.len() > 64) Self else format.request(input)

	result : FormatTimes, Engine.Result -> Try(List(System.FormattedTime), System.FormatFailure)
	result = |_, answer| match answer {
		PortAnswer(_) => match format.result(answer) {
			Ok({ code: 0, parts }) => Ok(parts)
			Ok({ code: 2, .. }) => Err(Refused)
			Ok({ code: 3, .. }) => Err(TooLarge)
			Ok({ code: 4, .. }) => Err(OutsideRange)
			_ => Err(Unavailable)
		}
		Now => Err(TooLarge)
		Refused | PortRefused(_) => Err(Refused)
		_ => Err(Unavailable)
	}
}

Random := {}.{
	request : Random, U64 -> Engine.Request
	request = |_, count| if (count > 65_536) Self else Crypto(RandomBytes(count))

	result : Random, Engine.Result -> Try(List(U8), System.RandomFailure)
	result = |_, answer| match answer {
		CryptoBytes(bytes) => Ok(bytes)
		Now | CryptoFailed(TooLarge) => Err(TooLarge)
		Refused | CryptoFailed(Refused) => Err(Refused)
		_ => Err(Unavailable)
	}
}

expect System.Mark.elapsed_ms_since(System.Mark.(12), System.Mark.(3)) == Ok(9)
expect System.Mark.elapsed_ms_since(System.Mark.(3), System.Mark.(12)) == Err(OutOfOrder)
