program osd_oracle
  implicit none
  integer, parameter :: N = 174
  character(len=512) :: in_path, out_path
  integer :: nvecs, iv, mode_op, k, ndeep_or_maxosd, norder
  integer :: ntype, nhard
  real :: dmin
  integer*1 :: apmask(N), cw(N), message91(91)
  real :: llr(N)

  if (command_argument_count() .lt. 2) then
     write(0,*) 'usage: osd_oracle <in_requests.bin> <out_responses.bin>'
     stop 1
  endif

  call get_command_argument(1, in_path)
  call get_command_argument(2, out_path)

  open(unit=10, file=trim(in_path), access='stream', form='unformatted', status='old')
  open(unit=11, file=trim(out_path), access='stream', form='unformatted', status='replace')

  read(10) nvecs
  write(11) nvecs

  do iv = 1, nvecs
     read(10) mode_op, k, ndeep_or_maxosd, norder, apmask, llr
     cw = 0
     message91 = 0
     ntype = 0
     nhard = -1
     dmin = 0.0
     if (mode_op .eq. 0) then
        call osd174_91(llr, k, apmask, ndeep_or_maxosd, message91, cw, nhard, dmin)
     else
        call decode174_91(llr, k, ndeep_or_maxosd, norder, apmask, message91, cw, ntype, nhard, dmin)
     endif
     write(11) int(ntype, 4), int(nhard, 4), dmin, cw, message91
  enddo

  close(10)
  close(11)
end program osd_oracle
