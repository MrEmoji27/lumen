--- Every link on each page of a list, as [text](url) lines per page.
--@input urls: list of URLs
--@input match: keep links whose text or URL has this (optional)
--@input max: links per page, default 100 (optional)

local pages = {}
for _, url in ipairs(input.urls or {}) do
	local tab = lumen.open(url)
	local out = lumen.call("browser_links", { tab = tab, match = input.match, max = input.max or 100 })
	local links = {}
	-- The links come fenced as page text: one [text](url) a line.
	for line in string.gmatch(out, "[^\n]+") do
		if string.sub(line, 1, 1) == "[" then
			table.insert(links, line)
		end
	end
	lumen.call("browser_close", { tab = tab })
	table.insert(pages, { url = url, links = links })
end
return pages
