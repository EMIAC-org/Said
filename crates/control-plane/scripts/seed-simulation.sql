-- Seed 3 dummy participants for meeting simulation.
-- Run: psql -d said_enterprise -f scripts/seed-simulation.sql

-- Password for all dummy users: "testpass1234" (argon2 hash below)
-- Generated via: python3 -c "from argon2 import PasswordHasher; print(PasswordHasher().hash('testpass1234'))"

DO $$
DECLARE
  v_org_id  UUID := '95d7b4b8-4b8b-434b-8e2c-c51447466e9b';
  v_hash    TEXT := '$argon2id$v=19$m=65536,t=3,p=4$8nwqqlTRJ4UnRKjeH2Ph2g$jo4YRXm0RNpLmi/oQrSZ/JVjzecwkSvNBlWRMGJq0FI';
  v_rahul   UUID;
  v_priya   UUID;
  v_anish   UUID;
BEGIN
  -- Rahul Kumar
  INSERT INTO accounts (email, password_hash)
    VALUES ('rahul@emiactech.com', v_hash)
    ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
    RETURNING id INTO v_rahul;
  IF v_rahul IS NULL THEN SELECT id INTO v_rahul FROM accounts WHERE email = 'rahul@emiactech.com'; END IF;

  INSERT INTO org_members (org_id, account_id, role, lark_name, lark_department)
    VALUES (v_org_id, v_rahul, 'MEMBER', 'Rahul Kumar', 'Engineering')
    ON CONFLICT (org_id, account_id) DO UPDATE SET lark_name = 'Rahul Kumar', lark_department = 'Engineering';

  INSERT INTO license_keys (account_id, tier, active)
    VALUES (v_rahul, 'free', true)
    ON CONFLICT DO NOTHING;

  -- Priya Sharma
  INSERT INTO accounts (email, password_hash)
    VALUES ('priya@emiactech.com', v_hash)
    ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
    RETURNING id INTO v_priya;
  IF v_priya IS NULL THEN SELECT id INTO v_priya FROM accounts WHERE email = 'priya@emiactech.com'; END IF;

  INSERT INTO org_members (org_id, account_id, role, lark_name, lark_department)
    VALUES (v_org_id, v_priya, 'MEMBER', 'Priya Sharma', 'Design')
    ON CONFLICT (org_id, account_id) DO UPDATE SET lark_name = 'Priya Sharma', lark_department = 'Design';

  INSERT INTO license_keys (account_id, tier, active)
    VALUES (v_priya, 'free', true)
    ON CONFLICT DO NOTHING;

  -- Anish Suman
  INSERT INTO accounts (email, password_hash)
    VALUES ('anish@emiactech.com', v_hash)
    ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
    RETURNING id INTO v_anish;
  IF v_anish IS NULL THEN SELECT id INTO v_anish FROM accounts WHERE email = 'anish@emiactech.com'; END IF;

  INSERT INTO org_members (org_id, account_id, role, lark_name, lark_department)
    VALUES (v_org_id, v_anish, 'MEMBER', 'Anish Suman', 'Engineering')
    ON CONFLICT (org_id, account_id) DO UPDATE SET lark_name = 'Anish Suman', lark_department = 'Engineering';

  INSERT INTO license_keys (account_id, tier, active)
    VALUES (v_anish, 'free', true)
    ON CONFLICT DO NOTHING;

  -- Update Abhishek's lark_name if not set
  UPDATE org_members SET lark_name = 'Abhishek Verma', lark_department = 'Engineering'
    WHERE org_id = v_org_id AND account_id = (SELECT id FROM accounts WHERE email = 'abhishek@emiactech.com');

  RAISE NOTICE 'Seeded: Rahul=%, Priya=%, Anish=%', v_rahul, v_priya, v_anish;
END $$;
