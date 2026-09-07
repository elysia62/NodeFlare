UPDATE servers SET price = 0 WHERE price < 0;

-- Preserve the existing table and its foreign keys when tightening the limit.
CREATE TRIGGER servers_price_nonnegative_insert
BEFORE INSERT ON servers WHEN NEW.price < 0
BEGIN
  SELECT RAISE(ABORT, 'price must be nonnegative');
END;

CREATE TRIGGER servers_price_nonnegative_update
BEFORE UPDATE OF price ON servers WHEN NEW.price < 0
BEGIN
  SELECT RAISE(ABORT, 'price must be nonnegative');
END;
