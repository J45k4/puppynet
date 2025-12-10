import { test, expect } from "bun:test"
import { patternMatcher } from "./pattern-matcher"

test("match with result", () => {
	const matcher = patternMatcher({
		"/user/:userId": (params) => `User ${params.userId}`,
		"/*": (params) => "Not found",
	})

	const result = matcher.match("/user/123")
	expect(result).toEqual({
		pattern: "/user/:userId",
		result: "User 123",
	})
	const result2 = matcher.match("/notfound")
	expect(result2).toEqual({
		pattern: "/*",
		result: "Not found",
	})
})
