UPDATE servers SET price = 0 WHERE price < 0;
ALTER TABLE servers ADD CONSTRAINT servers_price_nonnegative CHECK (price >= 0);
