-- Enterprise workspace identity (server URL + org name for metering / enforcement)
ALTER TABLE local_user ADD COLUMN enterprise_server_url TEXT;
ALTER TABLE local_user ADD COLUMN enterprise_org_name TEXT;
