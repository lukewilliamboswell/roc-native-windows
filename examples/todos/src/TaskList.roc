import pf.Win
import pf.Engine
import time.Calendar
import time.FixedOffset
import time.LocalDateTime
import time.PosixBoundary

## Pure task records and the optional due-date policy.
TaskList := [].{
	Task : { id : U64, title : Str, done : Bool, notes : Str, due_date : Win.Ui.DateSelection, due_time : Win.Ui.TimeSelection }

	with_date : Task, Win.Ui.DateSelection -> Task
	with_date = |task, date|
		match date {
			NoDate => { ..task, due_date: NoDate, due_time: NoTime }
			Date(_) => { ..task, due_date: date }
		}

	with_time : Task, Win.Ui.TimeSelection -> Task
	with_time = |task, time|
		match task.due_date {
			NoDate => task
			Date(_) => { ..task, due_time: time }
		}

	update : List(Task), U64, (Task -> Task) -> List(Task)
	update = |tasks, id, change| tasks.map(|task| if (task.id == id) change(task) else task)

	has_due_date : Win.Ui.DateSelection -> Bool
	has_due_date = |date|
		match date {
			NoDate => Bool.False
			Date(_) => Bool.True
		}

	summary : Task -> Str
	summary = |task|
		match (task.due_date, task.due_time) {
			(NoDate, _) => "No due date"
			(Date(date), NoTime) => "Due ${date.to_iso()}"
			(Date(date), Time(time)) => "Due ${date.to_iso()} at ${time.to_iso()}"
		}

	project_date : Engine.Zone, I64 -> Win.Ui.DateSelection
	project_date = |zone, now| match local_date(zone, now) {
		Ok(day) => Date(day)
		Err(_) => NoDate
	}

	due_summary : Task, Win.Ui.DateSelection -> Str
	due_summary = |task, today| match (task.due_date, today, task.due_time) {
		(Date(due), Date(day), Time(time)) if (due == day and !task.done) => "Due today at ${time.to_iso()}"
		(Date(due), Date(day), NoTime) if (due == day and !task.done) => "Due today"
		_ => summary(task)
	}

	ordered : List(Task) -> List(Task)
	ordered = |tasks| tasks.sort_with(
		|a, b| {
			key_a = schedule_key(a)
			key_b = schedule_key(b)
			if key_a < key_b {
				Before
			} else if key_a > key_b {
				After
			} else if a.id < b.id {
				Before
			} else if a.id > b.id {
				After
			} else {
				Same
			}
		},
	)

	remaining : List(Task) -> Str
	remaining = |tasks| "${tasks.keep_if(|task| !task.done).len().to_str()} remaining"

	completion : List(Task) -> Win.Ui.CheckState
	completion = |tasks|
		if tasks.is_empty() or tasks.all(|task| !task.done) {
			Unchecked
		} else if tasks.all(|task| task.done) {
			Checked
		} else {
			Mixed
		}
}

local_date : Engine.Zone, I64 -> Try(Win.Ui.CivilDate, [Unprojectable])
local_date = |zone, now| {
	minutes = zone.offset_at(now) ? |_| Unprojectable
	micros = (now.to_i128() * 1000).to_i64_try() ? |_| Unprojectable
	local = FixedOffset.project(
		FixedOffset.from_seconds(minutes * 60),
		PosixBoundary.from_microseconds(micros),
		Gregorian,
	) ? |_| Unprojectable
	fields = Calendar.Date.to_fields(LocalDateTime.date(local))
	year = fields.year.to_u16_try() ? |_| Unprojectable
	Win.Ui.CivilDate.from_parts({ year, month: fields.month, day: fields.day }).map_err(|_| Unprojectable)
}

schedule_key : TaskList.Task -> U64
schedule_key = |task| {
	date_key = match task.due_date {
		NoDate => 100000000
		Date(due) => {
			{ year, month, day } = due.parts()
			year.to_u64() * 10000 + month.to_u64() * 100 + day.to_u64()
		}
	}
	time_key = match task.due_time {
		NoTime => 1440
		Time(due) => {
			{ hour, minute } = due.parts()
			hour.to_u64() * 60 + minute.to_u64()
		}
	}
	date_key * 1441 + time_key
}

expect TaskList.remaining([]) == "0 remaining"

expect {
	first = { id: 1, title: "First", done: Bool.False, notes: "", due_date: NoDate, due_time: NoTime }
	second = { ..first, id: 2, title: "Second" }
	TaskList.update([first, second], 1, |task| { ..task, done: Bool.True }) == [{ ..first, done: Bool.True }, second]
}

expect {
	date = Win.Ui.CivilDate.from_parts({ year: 2028, month: 2, day: 29 })
	time = Win.Ui.CivilTime.from_parts({ hour: 9, minute: 15 })
	match (date, time) {
		(Ok(day), Ok(clock)) => {
			first = { id: 1, title: "First", done: Bool.False, notes: "", due_date: Date(day), due_time: Time(clock) }
			second = { ..first, id: 2, title: "Second", due_time: NoTime }
			TaskList.with_date(first, NoDate) == { ..first, due_date: NoDate, due_time: NoTime }
				and TaskList.ordered([second, first]) == [first, second]
				and TaskList.update([first, second], 1, |task| TaskList.with_date(task, NoDate)).last() == Ok(second)
		}
		_ => Bool.False
	}
}

expect Win.Ui.CivilDate.from_parts({ year: 2027, month: 2, day: 29 }) == Err(InvalidCalendarDate)
