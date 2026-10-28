(* Copies the scanner's 32 output bytes back as its 32 input bytes, like
   OpENer's sample application. scripts/adapter-smoke.sh expects this. *)
PROGRAM echo
  %QL0 := %IL0;
  %QL1 := %IL1;
  %QL2 := %IL2;
  %QL3 := %IL3;
END_PROGRAM
