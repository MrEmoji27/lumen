--- Search a shop, read the result pages you name, and put one in the cart.
--@input start: the shop's search page URL
--@input query: what to search for
--@input read: result titles to read, a list (each is opened, read and left)
--@input buy: the result title to put in the cart (optional)

local tab = lumen.open(input.start)
local snap = lumen.call("browser_snapshot", { tab = tab, interactive = true })
local box = lumen.ref(snap, "textbox") or error("no search box on " .. input.start)
lumen.call("browser_type", { tab = tab, ref = box, text = input.query, submit = true })

local found = {}
for _, title in ipairs(input.read or {}) do
	lumen.call("browser_wait", { tab = tab, text = title })
	local list = lumen.call("browser_snapshot", { tab = tab, interactive = true })
	local link = lumen.ref(list, title)
	if link then
		lumen.call("browser_click", { tab = tab, ref = link })
		local text = lumen.call("browser_article", { tab = tab })
		local price = string.match(text, "Price: ([%d%.]+)")
		local page = { title = title, price = price }
		if title == input.buy then
			local here = lumen.call("browser_snapshot", { tab = tab, interactive = true })
			local add = lumen.ref(here, "Add to cart")
			if add then
				lumen.call("browser_click", { tab = tab, ref = add })
				page.in_cart = true
			end
		end
		table.insert(found, page)
		lumen.call("browser_back", { tab = tab })
	else
		table.insert(found, { title = title, missing = true })
	end
end
lumen.call("browser_close", { tab = tab })
return found
