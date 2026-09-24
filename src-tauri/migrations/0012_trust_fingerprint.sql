-- What a project could run as agent configuration when the user trusted it
-- (`project::trust_fingerprint`). A different one later asks again. NULL on
-- rows trusted before this existed; the first open fills it in.
ALTER TABLE projects ADD COLUMN trust_fingerprint TEXT;
