(test "read both ends of the supported Gregorian calendar range"
  (steps
    (set-date (date-picker "New due date") (date 1753 1 1))
    (expect-date (date-picker "New due date") (date 1753 1 1))
    (set-date (date-picker "New due date") (date 9999 12 31))
    (expect-date (date-picker "New due date") (date 9999 12 31))
    (set-date (date-picker "New due date") no-date)
    (expect-date (date-picker "New due date") no-date)))
